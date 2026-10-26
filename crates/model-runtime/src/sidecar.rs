//! Supervision for the bundled `llama-server` process.

use std::{
    error::Error,
    fmt,
    net::{Ipv4Addr, SocketAddrV4, TcpListener},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{Mutex, MutexGuard},
};

const LOOPBACK_ADDRESS: Ipv4Addr = Ipv4Addr::LOCALHOST;
const API_CREDENTIAL_BYTES: usize = 32;
const DEFAULT_CONTEXT_TOKENS: u32 = 32_768;
const MAX_CONTEXT_TOKENS: u32 = 1_048_576;
pub(crate) const MEDIA_MARKER: &str = "<__zeta_image__>";

/// Launch configuration for one verified local model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LlamaSidecarConfig {
    executable: PathBuf,
    model: PathBuf,
    context_tokens: u32,
    multimodal: Option<MultimodalSidecarConfig>,
}

/// Verified local projector and explicit Glimmer image-token bounds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MultimodalSidecarConfig {
    projector: PathBuf,
    image_tokens: u32,
}

impl LlamaSidecarConfig {
    /// Creates a launch configuration for the bundled helper and one local model.
    #[must_use]
    pub fn new(executable: impl Into<PathBuf>, model: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            model: model.into(),
            context_tokens: DEFAULT_CONTEXT_TOKENS,
            multimodal: None,
        }
    }

    /// Applies the Rust-enforced server context allocation for this model.
    ///
    /// # Errors
    ///
    /// Returns an error for a zero or unreasonably large context.
    pub fn with_context_tokens(mut self, context_tokens: u32) -> Result<Self, SidecarError> {
        if context_tokens == 0 || context_tokens > MAX_CONTEXT_TOKENS {
            return Err(SidecarError::InvalidContextSize);
        }
        if self
            .multimodal
            .as_ref()
            .is_some_and(|profile| profile.image_tokens >= context_tokens)
        {
            return Err(SidecarError::InvalidImageTokenLimit);
        }
        self.context_tokens = context_tokens;
        Ok(self)
    }

    /// Enables a local-only multimodal profile using one verified projector.
    ///
    /// # Errors
    ///
    /// Returns an error when the fixed per-image token budget is zero or cannot
    /// fit inside the configured server context.
    pub fn with_multimodal_projector(
        mut self,
        projector: impl Into<PathBuf>,
        image_tokens: u32,
    ) -> Result<Self, SidecarError> {
        if image_tokens == 0 || image_tokens >= self.context_tokens {
            return Err(SidecarError::InvalidImageTokenLimit);
        }
        self.multimodal = Some(MultimodalSidecarConfig {
            projector: projector.into(),
            image_tokens,
        });
        Ok(self)
    }

    /// Returns the configured helper path.
    #[must_use]
    pub fn executable(&self) -> &Path {
        &self.executable
    }

    /// Returns the configured model path.
    #[must_use]
    pub fn model(&self) -> &Path {
        &self.model
    }

    /// Returns the total context allocated by the helper.
    #[must_use]
    pub const fn context_tokens(&self) -> u32 {
        self.context_tokens
    }

    /// Returns the enabled multimodal launch profile, when present.
    #[must_use]
    pub const fn multimodal(&self) -> Option<&MultimodalSidecarConfig> {
        self.multimodal.as_ref()
    }
}

/// A per-launch credential used to authenticate inference requests.
#[derive(Clone, Eq, PartialEq)]
pub struct ApiCredential([u8; API_CREDENTIAL_BYTES]);

impl ApiCredential {
    fn generate() -> Result<Self, SidecarError> {
        let mut bytes = [0_u8; API_CREDENTIAL_BYTES];
        getrandom::fill(&mut bytes).map_err(SidecarError::CredentialGeneration)?;
        Ok(Self(bytes))
    }

    /// Returns the credential encoded for `llama-server`'s bearer-token interface.
    ///
    /// Callers must not log, persist or expose the returned value to the frontend.
    #[must_use]
    pub fn expose_hex(&self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut encoded = String::with_capacity(API_CREDENTIAL_BYTES * 2);
        for byte in self.0 {
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        encoded
    }
}

impl fmt::Debug for ApiCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ApiCredential([REDACTED])")
    }
}

impl Drop for ApiCredential {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

/// Authenticated loopback endpoint for a running helper.
#[derive(Clone, Eq, PartialEq)]
pub struct SidecarEndpoint {
    address: SocketAddrV4,
    credential: ApiCredential,
}

impl SidecarEndpoint {
    /// Returns the IPv4 loopback address selected for this launch.
    #[must_use]
    pub const fn address(&self) -> SocketAddrV4 {
        self.address
    }

    /// Returns the credential needed by the Rust inference client.
    #[must_use]
    pub const fn credential(&self) -> &ApiCredential {
        &self.credential
    }

    #[cfg(test)]
    pub(crate) fn test_endpoint(address: SocketAddrV4) -> Result<Self, SidecarError> {
        Ok(Self {
            address,
            credential: ApiCredential::generate()?,
        })
    }
}

impl fmt::Debug for SidecarEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SidecarEndpoint")
            .field("address", &self.address)
            .field("credential", &"[REDACTED]")
            .finish()
    }
}

/// Observable state of the supervised helper.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SidecarStatus {
    /// No helper is running.
    Stopped,
    /// The helper child process is running.
    Running,
    /// The helper exited without an explicit stop request.
    Exited {
        /// Platform exit code, when one was available.
        code: Option<i32>,
    },
}

#[derive(Debug, Default)]
struct SupervisorState {
    process: Option<Child>,
    endpoint: Option<SidecarEndpoint>,
    last_exit: Option<ExitStatus>,
}

/// Rust-owned lifecycle supervisor for the bundled `llama-server`.
///
/// Dropping this value synchronously terminates and reaps a running child.
#[derive(Debug, Default)]
pub struct LlamaSidecar {
    state: Mutex<SupervisorState>,
}

impl LlamaSidecar {
    /// Creates an idle supervisor.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts the bundled server for one local model.
    ///
    /// The helper binds to an ephemeral IPv4 loopback port, runs in offline
    /// mode, disables request logging and its web UI, and receives its fresh
    /// API key through a cleared child environment rather than process args.
    ///
    /// # Errors
    ///
    /// Returns an error for missing artifacts, unavailable local ports, random
    /// credential failure, duplicate launches or child-process launch failure.
    pub fn start(&self, config: &LlamaSidecarConfig) -> Result<SidecarEndpoint, SidecarError> {
        let mut state = self.lock_state()?;
        refresh_state(&mut state)?;
        if state.process.is_some() {
            return Err(SidecarError::AlreadyRunning);
        }

        let prepared = PreparedLaunch::new(config)?;
        let endpoint = prepared.endpoint.clone();
        let child = prepared.spawn()?;
        state.process = Some(child);
        state.endpoint = Some(endpoint.clone());
        state.last_exit = None;
        Ok(endpoint)
    }

    /// Returns the current process state and detects unexpected exits.
    ///
    /// # Errors
    ///
    /// Returns an error when the supervisor lock or child status is unavailable.
    pub fn status(&self) -> Result<SidecarStatus, SidecarError> {
        let mut state = self.lock_state()?;
        refresh_state(&mut state)?;
        if state.process.is_some() {
            Ok(SidecarStatus::Running)
        } else if let Some(exit) = state.last_exit {
            Ok(SidecarStatus::Exited { code: exit.code() })
        } else {
            Ok(SidecarStatus::Stopped)
        }
    }

    /// Returns the active authenticated endpoint, if the helper is still running.
    ///
    /// # Errors
    ///
    /// Returns an error when the supervisor lock or child status is unavailable.
    pub fn endpoint(&self) -> Result<Option<SidecarEndpoint>, SidecarError> {
        let mut state = self.lock_state()?;
        refresh_state(&mut state)?;
        Ok(state.endpoint.clone())
    }

    /// Terminates and reaps the running helper, if any.
    ///
    /// # Errors
    ///
    /// Returns an error if the child cannot be inspected, killed or reaped.
    pub fn stop(&self) -> Result<(), SidecarError> {
        let mut state = self.lock_state()?;
        terminate(&mut state)
    }

    fn lock_state(&self) -> Result<MutexGuard<'_, SupervisorState>, SidecarError> {
        self.state
            .lock()
            .map_err(|_| SidecarError::SupervisorUnavailable)
    }
}

impl Drop for LlamaSidecar {
    fn drop(&mut self) {
        let Ok(state) = self.state.get_mut() else {
            return;
        };
        let _ = terminate(state);
    }
}

struct PreparedLaunch {
    listener: TcpListener,
    endpoint: SidecarEndpoint,
    command: Command,
}

impl PreparedLaunch {
    fn new(config: &LlamaSidecarConfig) -> Result<Self, SidecarError> {
        let executable = canonical_file(config.executable(), ArtifactKind::Executable)?;
        let model = canonical_file(config.model(), ArtifactKind::Model)?;
        let listener =
            TcpListener::bind(SocketAddrV4::new(LOOPBACK_ADDRESS, 0)).map_err(|source| {
                SidecarError::Io {
                    operation: "reserve a loopback port",
                    source,
                }
            })?;
        let address = match listener.local_addr().map_err(|source| SidecarError::Io {
            operation: "inspect the loopback port",
            source,
        })? {
            std::net::SocketAddr::V4(address) if *address.ip() == LOOPBACK_ADDRESS => address,
            _ => return Err(SidecarError::NonLoopbackEndpoint),
        };
        let credential = ApiCredential::generate()?;
        let endpoint = SidecarEndpoint {
            address,
            credential,
        };
        let projector = config
            .multimodal()
            .map(|profile| canonical_file(&profile.projector, ArtifactKind::Projector))
            .transpose()?;
        let command = server_command(
            &executable,
            &model,
            config.context_tokens(),
            projector.as_deref().zip(config.multimodal()),
            &endpoint,
        );
        Ok(Self {
            listener,
            endpoint,
            command,
        })
    }

    fn spawn(mut self) -> Result<Child, SidecarError> {
        // `llama-server` cannot inherit a pre-bound listener. Keep the port
        // reserved until immediately before spawn to minimize the local race.
        drop(self.listener);
        self.command.spawn().map_err(|source| SidecarError::Io {
            operation: "launch llama-server",
            source,
        })
    }
}

fn server_command(
    executable: &Path,
    model: &Path,
    context_tokens: u32,
    multimodal: Option<(&Path, &MultimodalSidecarConfig)>,
    endpoint: &SidecarEndpoint,
) -> Command {
    let mut command = Command::new(executable);
    command
        .arg("--model")
        .arg(model)
        .arg("--host")
        .arg(LOOPBACK_ADDRESS.to_string())
        .arg("--port")
        .arg(endpoint.address.port().to_string())
        .arg("--ctx-size")
        .arg(context_tokens.to_string())
        .arg("--parallel")
        .arg("1")
        .arg("--special")
        .arg("--offline")
        .arg("--log-disable")
        .arg("--no-webui")
        .arg("--no-slots")
        .env_clear()
        .env("LLAMA_API_KEY", endpoint.credential.expose_hex())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some((projector, profile)) = multimodal {
        command
            .arg("--mmproj")
            .arg(projector)
            .arg("--image-min-tokens")
            .arg(profile.image_tokens.to_string())
            .arg("--image-max-tokens")
            .arg(profile.image_tokens.to_string())
            .arg("--mtmd-batch-max-tokens")
            .arg(profile.image_tokens.to_string())
            .env("LLAMA_MEDIA_MARKER", MEDIA_MARKER);
    }
    command
}

fn canonical_file(path: &Path, kind: ArtifactKind) -> Result<PathBuf, SidecarError> {
    let canonical = path.canonicalize().map_err(|source| SidecarError::Io {
        operation: kind.canonicalize_operation(),
        source,
    })?;
    if !canonical.is_file() {
        return Err(match kind {
            ArtifactKind::Executable => SidecarError::InvalidExecutable(canonical),
            ArtifactKind::Model => SidecarError::InvalidModel(canonical),
            ArtifactKind::Projector => SidecarError::InvalidProjector(canonical),
        });
    }
    Ok(canonical)
}

fn refresh_state(state: &mut SupervisorState) -> Result<(), SidecarError> {
    let Some(process) = state.process.as_mut() else {
        return Ok(());
    };
    let exit = process.try_wait().map_err(|source| SidecarError::Io {
        operation: "inspect llama-server",
        source,
    })?;
    if let Some(exit) = exit {
        state.process = None;
        state.endpoint = None;
        state.last_exit = Some(exit);
    }
    Ok(())
}

fn terminate(state: &mut SupervisorState) -> Result<(), SidecarError> {
    let Some(mut process) = state.process.take() else {
        state.endpoint = None;
        state.last_exit = None;
        return Ok(());
    };
    if process
        .try_wait()
        .map_err(|source| SidecarError::Io {
            operation: "inspect llama-server before shutdown",
            source,
        })?
        .is_none()
    {
        process.kill().map_err(|source| SidecarError::Io {
            operation: "terminate llama-server",
            source,
        })?;
    }
    process.wait().map_err(|source| SidecarError::Io {
        operation: "reap llama-server",
        source,
    })?;
    state.endpoint = None;
    state.last_exit = None;
    Ok(())
}

#[derive(Clone, Copy)]
enum ArtifactKind {
    Executable,
    Model,
    Projector,
}

impl ArtifactKind {
    const fn canonicalize_operation(self) -> &'static str {
        match self {
            Self::Executable => "resolve the llama-server executable",
            Self::Model => "resolve the model artifact",
            Self::Projector => "resolve the vision projector",
        }
    }
}

/// Failure while launching or supervising local inference.
#[derive(Debug)]
pub enum SidecarError {
    /// The configured helper is not a regular file.
    InvalidExecutable(PathBuf),
    /// The configured model is not a regular file.
    InvalidModel(PathBuf),
    /// The configured vision projector is not a regular file.
    InvalidProjector(PathBuf),
    /// A helper is already active.
    AlreadyRunning,
    /// The requested server context allocation is invalid.
    InvalidContextSize,
    /// The per-image token budget is invalid for the selected context.
    InvalidImageTokenLimit,
    /// The operating system returned an unexpected non-loopback address.
    NonLoopbackEndpoint,
    /// Secure random credential generation failed.
    CredentialGeneration(getrandom::Error),
    /// The supervisor lock is poisoned and cannot be trusted.
    SupervisorUnavailable,
    /// An operating-system operation failed.
    Io {
        /// Stable operation description that contains no path or secret.
        operation: &'static str,
        /// Underlying I/O error.
        source: std::io::Error,
    },
}

impl fmt::Display for SidecarError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidExecutable(_) => formatter.write_str("llama-server is not a regular file"),
            Self::InvalidModel(_) => {
                formatter.write_str("the selected model is not a regular file")
            }
            Self::InvalidProjector(_) => {
                formatter.write_str("the selected vision projector is not a regular file")
            }
            Self::AlreadyRunning => formatter.write_str("llama-server is already running"),
            Self::InvalidContextSize => {
                formatter.write_str("the llama-server context size is invalid")
            }
            Self::InvalidImageTokenLimit => {
                formatter.write_str("the multimodal image-token limit is invalid")
            }
            Self::NonLoopbackEndpoint => {
                formatter.write_str("the operating system did not allocate a loopback endpoint")
            }
            Self::CredentialGeneration(_) => {
                formatter.write_str("could not generate an inference credential")
            }
            Self::SupervisorUnavailable => {
                formatter.write_str("the inference supervisor is unavailable")
            }
            Self::Io { operation, .. } => write!(formatter, "could not {operation}"),
        }
    }
}

impl Error for SidecarError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::CredentialGeneration(source) => Some(source),
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsStr, fs};

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn credentials_are_fresh_and_redacted() -> Result<(), Box<dyn Error>> {
        let first = ApiCredential::generate()?;
        let second = ApiCredential::generate()?;

        assert_ne!(first, second);
        assert_eq!(first.expose_hex().len(), API_CREDENTIAL_BYTES * 2);
        assert!(
            first
                .expose_hex()
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        );
        assert_eq!(format!("{first:?}"), "ApiCredential([REDACTED])");
        Ok(())
    }

    #[test]
    fn launch_is_loopback_offline_authenticated_and_log_free() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let executable = directory.path().join("llama-server");
        let model = directory.path().join("model.gguf");
        fs::write(&executable, b"test helper")?;
        fs::write(&model, b"test model")?;
        let config = LlamaSidecarConfig::new(&executable, &model);
        let prepared = PreparedLaunch::new(&config)?;
        let args: Vec<_> = prepared.command.get_args().collect();
        let token = prepared.endpoint.credential.expose_hex();
        let default_context = DEFAULT_CONTEXT_TOKENS.to_string();

        assert_eq!(*prepared.endpoint.address.ip(), Ipv4Addr::LOCALHOST);
        assert!(
            args.windows(2)
                .any(|pair| { pair == [OsStr::new("--host"), OsStr::new("127.0.0.1")] })
        );
        assert!(args.contains(&OsStr::new("--offline")));
        assert!(args.contains(&OsStr::new("--log-disable")));
        assert!(args.contains(&OsStr::new("--no-webui")));
        assert!(args.contains(&OsStr::new("--special")));
        assert!(
            args.windows(2)
                .any(|pair| { pair == [OsStr::new("--ctx-size"), OsStr::new(&default_context),] })
        );
        assert!(
            args.windows(2)
                .any(|pair| { pair == [OsStr::new("--parallel"), OsStr::new("1")] })
        );
        assert!(!args.iter().any(|arg| *arg == OsStr::new(&token)));
        assert!(prepared.command.get_envs().any(|(key, value)| {
            key == OsStr::new("LLAMA_API_KEY") && value == Some(OsStr::new(&token))
        }));
        assert!(!args.contains(&OsStr::new("--mmproj")));
        assert!(
            !prepared
                .command
                .get_envs()
                .any(|(key, _)| { key == OsStr::new("LLAMA_MEDIA_MARKER") })
        );
        Ok(())
    }

    #[test]
    fn multimodal_launch_uses_only_local_projector_and_fixed_token_limits()
    -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let executable = directory.path().join("llama-server");
        let model = directory.path().join("model.gguf");
        let projector = directory.path().join("projector.gguf");
        fs::write(&executable, b"test helper")?;
        fs::write(&model, b"test model")?;
        fs::write(&projector, b"test projector")?;
        let config = LlamaSidecarConfig::new(&executable, &model)
            .with_multimodal_projector(&projector, 1_024)?;
        let prepared = PreparedLaunch::new(&config)?;
        let args = prepared.command.get_args().collect::<Vec<_>>();
        let canonical_projector = projector.canonicalize()?;

        for (flag, value) in [
            ("--image-min-tokens", "1024"),
            ("--image-max-tokens", "1024"),
            ("--mtmd-batch-max-tokens", "1024"),
        ] {
            assert!(
                args.windows(2)
                    .any(|pair| { pair == [OsStr::new(flag), OsStr::new(value)] })
            );
        }
        assert!(
            args.windows(2)
                .any(|pair| { pair == [OsStr::new("--mmproj"), canonical_projector.as_os_str()] })
        );
        assert!(!args.contains(&OsStr::new("--media-path")));
        assert!(prepared.command.get_envs().any(|(key, value)| {
            key == OsStr::new("LLAMA_MEDIA_MARKER") && value == Some(OsStr::new(MEDIA_MARKER))
        }));
        Ok(())
    }

    #[test]
    fn context_allocation_is_explicit_and_bounded() -> Result<(), Box<dyn Error>> {
        let config = LlamaSidecarConfig::new("helper", "model").with_context_tokens(8_192)?;
        assert_eq!(config.context_tokens(), 8_192);
        assert!(matches!(
            LlamaSidecarConfig::new("helper", "model").with_context_tokens(0),
            Err(SidecarError::InvalidContextSize)
        ));
        Ok(())
    }

    #[test]
    fn stop_terminates_and_reaps_the_child() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let model = directory.path().join("model.gguf");
        fs::write(&model, b"test model")?;
        let supervisor = LlamaSidecar::new();
        let config = LlamaSidecarConfig::new("/usr/bin/yes", model);

        supervisor.start(&config)?;
        assert_eq!(supervisor.status()?, SidecarStatus::Running);
        supervisor.stop()?;
        assert_eq!(supervisor.status()?, SidecarStatus::Stopped);
        Ok(())
    }
}
