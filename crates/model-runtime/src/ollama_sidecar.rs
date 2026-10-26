//! Supervision for Zeta's pinned, private Ollama runtime.

use std::{
    error::Error,
    fmt, fs,
    io::Read,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use reqwest::blocking::Client;
#[cfg(unix)]
use rustix::process::{Pid, Signal, kill_process_group, test_kill_process_group};
use serde::Deserialize;

#[cfg(unix)]
use std::os::unix::process::CommandExt as _;

use crate::blocking_client::BlockingClient;

mod acceleration;
#[cfg(debug_assertions)]
mod cache_telemetry;

/// Ollama release qualified for the bundled Qwen provider.
pub const PINNED_OLLAMA_VERSION: &str = "0.32.14";

const LOOPBACK_ADDRESS: Ipv4Addr = Ipv4Addr::LOCALHOST;
const STARTUP_DEADLINE: Duration = Duration::from_secs(20);
const HEALTH_REQUEST_TIMEOUT: Duration = Duration::from_secs(1);
const HEALTH_RETRY_DELAY: Duration = Duration::from_millis(50);
const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_START_ATTEMPTS: usize = 3;

/// Private native-shell mode used to supervise Ollama across parent death.
pub const OLLAMA_GUARDIAN_ARGUMENT: &str = "--zeta-ollama-guardian";

/// Launch configuration for the bundled Ollama process and Zeta-private store.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OllamaSidecarConfig {
    executable: PathBuf,
    models_directory: PathBuf,
    guardian_executable: Option<PathBuf>,
    qwen_acceleration: bool,
    #[cfg(debug_assertions)]
    cache_telemetry_directory: Option<PathBuf>,
}

impl OllamaSidecarConfig {
    /// Creates a configuration for the pinned runtime payload.
    #[must_use]
    pub fn new(executable: impl Into<PathBuf>, models_directory: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            models_directory: models_directory.into(),
            guardian_executable: None,
            qwen_acceleration: false,
            #[cfg(debug_assertions)]
            cache_telemetry_directory: None,
        }
    }

    /// Routes the server through a copy of Zeta's native executable.
    ///
    /// The guardian owns Ollama and observes an inherited pipe. Abrupt native
    /// process exit closes that pipe, allowing the guardian to terminate and
    /// reap Ollama even when ordinary Rust destructors cannot run.
    #[must_use]
    pub fn with_guardian_executable(mut self, executable: impl Into<PathBuf>) -> Self {
        self.guardian_executable = Some(executable.into());
        self
    }

    /// Selects the separately pinned bundled Qwen payload after host hardware admission.
    /// The caller must establish the measured M1 Pro profile; other hosts retain stock.
    #[must_use]
    pub const fn with_qwen_acceleration(mut self) -> Self {
        self.qwen_acceleration = true;
        self
    }

    /// Enables content-free backend cache events for a guarded development run.
    #[cfg(debug_assertions)]
    #[must_use]
    pub fn with_cache_telemetry_directory(mut self, directory: impl Into<PathBuf>) -> Self {
        self.cache_telemetry_directory = Some(directory.into());
        self
    }

    /// Returns the bundled server executable path.
    #[must_use]
    pub fn executable(&self) -> &Path {
        &self.executable
    }

    /// Returns the private Ollama model-store path.
    #[must_use]
    pub fn models_directory(&self) -> &Path {
        &self.models_directory
    }
}

/// Unauthenticated loopback endpoint owned by the native process.
///
/// Ollama has no per-launch bearer-token facility. This value is deliberately
/// not presented as authenticated; callers must keep it inside the Rust
/// process and use only Zeta's closed provider operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OllamaSidecarEndpoint {
    address: SocketAddrV4,
}

impl OllamaSidecarEndpoint {
    /// Returns the ephemeral IPv4 loopback address selected for this launch.
    #[must_use]
    pub const fn address(self) -> SocketAddrV4 {
        self.address
    }
}

/// Observable lifecycle state of the bundled Ollama process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OllamaSidecarStatus {
    /// No child is running.
    Stopped,
    /// The child passed its bounded version health check and is running.
    Running,
    /// The child exited without an explicit stop request.
    Exited {
        /// Platform exit code, when available.
        code: Option<i32>,
    },
}

#[derive(Debug, Default)]
struct SupervisorState {
    process: Option<Child>,
    graceful_shutdown: bool,
    endpoint: Option<OllamaSidecarEndpoint>,
    last_exit: Option<ExitStatus>,
}

/// Rust-owned lifecycle supervisor for the bundled Ollama server.
#[derive(Debug, Default)]
pub struct OllamaSidecar {
    state: Mutex<SupervisorState>,
}

impl OllamaSidecar {
    /// Creates an idle supervisor.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts Ollama, then waits for an exact pinned-version health response.
    ///
    /// # Errors
    ///
    /// Returns a sanitized error for invalid artifacts, duplicate launch,
    /// process failure, incompatible version or startup timeout.
    pub fn start(
        &self,
        config: &OllamaSidecarConfig,
    ) -> Result<OllamaSidecarEndpoint, OllamaSidecarError> {
        let mut state = self.lock_state()?;
        refresh_state(&mut state)?;
        if state.process.is_some() {
            return Err(OllamaSidecarError::AlreadyRunning);
        }

        for attempt in 0..MAX_START_ATTEMPTS {
            let prepared = PreparedLaunch::new(config)?;
            let endpoint = prepared.endpoint;
            let graceful_shutdown = prepared.graceful_shutdown;
            let child = prepared.spawn()?;
            state.process = Some(child);
            state.graceful_shutdown = graceful_shutdown;
            state.endpoint = Some(endpoint);
            state.last_exit = None;

            match await_healthy(&mut state, endpoint) {
                Ok(()) => return Ok(endpoint),
                Err(OllamaSidecarError::ExitedDuringStartup)
                    if attempt + 1 < MAX_START_ATTEMPTS =>
                {
                    terminate(&mut state)?;
                }
                Err(error) => {
                    let _ = terminate(&mut state);
                    return Err(error);
                }
            }
        }
        Err(OllamaSidecarError::ExitedDuringStartup)
    }

    /// Returns the current child state and detects unexpected exits.
    ///
    /// # Errors
    ///
    /// Returns an error if the lock or process status is unavailable.
    pub fn status(&self) -> Result<OllamaSidecarStatus, OllamaSidecarError> {
        let mut state = self.lock_state()?;
        refresh_state(&mut state)?;
        if state.process.is_some() {
            Ok(OllamaSidecarStatus::Running)
        } else if let Some(exit) = state.last_exit {
            Ok(OllamaSidecarStatus::Exited { code: exit.code() })
        } else {
            Ok(OllamaSidecarStatus::Stopped)
        }
    }

    /// Returns the active endpoint when the child is still running.
    ///
    /// # Errors
    ///
    /// Returns an error if the lock or process status is unavailable.
    pub fn endpoint(&self) -> Result<Option<OllamaSidecarEndpoint>, OllamaSidecarError> {
        let mut state = self.lock_state()?;
        refresh_state(&mut state)?;
        Ok(state.endpoint)
    }

    /// Terminates and synchronously reaps the child, if any.
    ///
    /// # Errors
    ///
    /// Returns an error if the child cannot be inspected, killed or reaped.
    pub fn stop(&self) -> Result<(), OllamaSidecarError> {
        let mut state = self.lock_state()?;
        terminate(&mut state)
    }

    fn lock_state(&self) -> Result<MutexGuard<'_, SupervisorState>, OllamaSidecarError> {
        self.state
            .lock()
            .map_err(|_| OllamaSidecarError::SupervisorUnavailable)
    }
}

impl Drop for OllamaSidecar {
    fn drop(&mut self) {
        if let Ok(state) = self.state.get_mut() {
            let _ = terminate(state);
        }
    }
}

struct PreparedLaunch {
    listener: TcpListener,
    endpoint: OllamaSidecarEndpoint,
    command: Command,
    graceful_shutdown: bool,
}

impl PreparedLaunch {
    fn new(config: &OllamaSidecarConfig) -> Result<Self, OllamaSidecarError> {
        let executable = if config.qwen_acceleration {
            acceleration::executable(config.executable())?
        } else {
            canonical_file(config.executable())?
        };
        let models_directory = prepare_private_directory(config.models_directory())?;
        let listener =
            TcpListener::bind(SocketAddrV4::new(LOOPBACK_ADDRESS, 0)).map_err(|source| {
                OllamaSidecarError::Io {
                    operation: "reserve an Ollama loopback port",
                    source,
                }
            })?;
        let address = match listener
            .local_addr()
            .map_err(|source| OllamaSidecarError::Io {
                operation: "inspect the Ollama loopback port",
                source,
            })? {
            SocketAddr::V4(address) if *address.ip() == LOOPBACK_ADDRESS => address,
            _ => return Err(OllamaSidecarError::NonLoopbackEndpoint),
        };
        let endpoint = OllamaSidecarEndpoint { address };
        let (mut command, graceful_shutdown) = launch_command(
            &executable,
            &models_directory,
            endpoint,
            config.guardian_executable.as_deref(),
        )?;
        if config.qwen_acceleration {
            acceleration::configure(&mut command);
        }
        #[cfg(debug_assertions)]
        if let (Some(directory), Some(_)) = (
            config.cache_telemetry_directory.as_ref(),
            config.guardian_executable.as_ref(),
        ) {
            command
                .env("ZETA_DEV_OLLAMA_CACHE_TELEMETRY_DIRECTORY", directory)
                .env("OLLAMA_DEBUG", "1")
                .env("OLLAMA_DEBUG_LOG_REQUESTS", "false");
        }
        Ok(Self {
            listener,
            endpoint,
            command,
            graceful_shutdown,
        })
    }

    fn spawn(mut self) -> Result<Child, OllamaSidecarError> {
        drop(self.listener);
        self.command
            .spawn()
            .map_err(|source| OllamaSidecarError::Io {
                operation: "launch bundled Ollama",
                source,
            })
    }
}

fn launch_command(
    executable: &Path,
    models_directory: &Path,
    endpoint: OllamaSidecarEndpoint,
    guardian_executable: Option<&Path>,
) -> Result<(Command, bool), OllamaSidecarError> {
    let guardian_executable = guardian_executable.map(canonical_file).transpose()?;
    let graceful_shutdown = guardian_executable.is_some();
    let mut command = if let Some(guardian) = guardian_executable.as_ref() {
        let mut command = Command::new(guardian);
        command
            .arg(OLLAMA_GUARDIAN_ARGUMENT)
            .arg(executable)
            .stdin(Stdio::piped());
        command
    } else {
        let mut command = Command::new(executable);
        command.arg("serve").stdin(Stdio::null());
        command
    };
    if let Some(runtime_directory) = executable.parent() {
        command.current_dir(runtime_directory);
    }
    command
        .env_clear()
        // Ollama requires HOME. Keep it inside Zeta's private runtime store
        // rather than inheriting the user's account directory.
        .env("HOME", models_directory)
        .env("OLLAMA_HOST", endpoint.address.to_string())
        .env("OLLAMA_MODELS", models_directory)
        .env("OLLAMA_NO_CLOUD", "1")
        // External registrations may point at pre-existing private-store files.
        // Provider garbage collection must never delete those user-owned weights.
        .env("OLLAMA_NOPRUNE", "1")
        // Exact preflight rendering and the immediately following generation
        // reuse one resident model. The one-model scheduler still unloads it
        // automatically when the user switches profiles.
        .env("OLLAMA_KEEP_ALIVE", "5m")
        .env("OLLAMA_MAX_LOADED_MODELS", "1")
        .env("OLLAMA_MAX_QUEUE", "1")
        .env(
            "OLLAMA_NUM_PARALLEL",
            crate::ollama::OLLAMA_PARALLEL_CONTEXTS.to_string(),
        )
        .env("OLLAMA_ORIGINS", "")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    Ok((command, graceful_shutdown))
}

/// Runs the native-shell guardian mode for one already-validated launch.
///
/// The caller must invoke this only for the exact private guardian argument.
/// The process inherits the supervisor's cleared, allow-listed environment.
/// Closing standard input is the ownership signal, including when the native
/// parent crashes or is force-quit.
///
/// # Errors
///
/// Returns an error when Ollama cannot be launched, inspected, terminated or
/// reaped.
pub fn run_ollama_guardian(executable: &Path) -> Result<(), OllamaSidecarError> {
    run_ollama_guardian_with_reader(executable, std::io::stdin())
}

fn run_ollama_guardian_with_reader(
    executable: &Path,
    mut ownership: impl Read + Send + 'static,
) -> Result<(), OllamaSidecarError> {
    let executable = canonical_file(executable)?;
    let ownership_closed = Arc::new(AtomicBool::new(false));
    let reader_signal = Arc::clone(&ownership_closed);
    thread::Builder::new()
        .name("zeta-ollama-owner-watch".to_owned())
        .spawn(move || {
            let mut byte = [0_u8; 1];
            loop {
                match ownership.read(&mut byte) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            reader_signal.store(true, Ordering::Release);
        })
        .map_err(|source| OllamaSidecarError::Io {
            operation: "start the Ollama ownership monitor",
            source,
        })?;
    let mut command = Command::new(&executable);
    if let Some(runtime_directory) = executable.parent() {
        command.current_dir(runtime_directory);
    }
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(debug_assertions)]
    let cache_telemetry_directory =
        std::env::var_os("ZETA_DEV_OLLAMA_CACHE_TELEMETRY_DIRECTORY").map(PathBuf::from);
    #[cfg(debug_assertions)]
    let capture_cache_telemetry = cache_telemetry_directory.is_some();
    let mut ollama = command
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr({
            #[cfg(debug_assertions)]
            if capture_cache_telemetry {
                Stdio::piped()
            } else {
                Stdio::null()
            }
            #[cfg(not(debug_assertions))]
            Stdio::null()
        })
        .spawn()
        .map_err(|source| OllamaSidecarError::Io {
            operation: "launch guarded Ollama",
            source,
        })?;
    #[cfg(debug_assertions)]
    let _cache_reader = if let (Some(stderr), Some(directory)) =
        (ollama.stderr.take(), cache_telemetry_directory)
    {
        cache_telemetry::capture(stderr, directory)
    } else {
        None
    };
    loop {
        if ollama
            .try_wait()
            .map_err(|source| OllamaSidecarError::Io {
                operation: "inspect guarded Ollama",
                source,
            })?
            .is_some()
        {
            return Ok(());
        }
        if ownership_closed.load(Ordering::Acquire) {
            terminate_guarded_ollama(&mut ollama)?;
            return Ok(());
        }
        thread::sleep(HEALTH_RETRY_DELAY);
    }
}

#[cfg(unix)]
fn terminate_guarded_ollama(ollama: &mut Child) -> Result<(), OllamaSidecarError> {
    let raw_pid = i32::try_from(ollama.id()).map_err(|source| OllamaSidecarError::Io {
        operation: "identify guarded Ollama's process group",
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, source),
    })?;
    let process_group = Pid::from_raw(raw_pid).ok_or_else(|| OllamaSidecarError::Io {
        operation: "identify guarded Ollama's process group",
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid child process ID"),
    })?;
    signal_ollama_process_group(process_group, Signal::TERM)?;
    let deadline = Instant::now() + GRACEFUL_SHUTDOWN_TIMEOUT;
    let mut child_exited = false;
    loop {
        if !child_exited {
            child_exited = ollama
                .try_wait()
                .map_err(|source| OllamaSidecarError::Io {
                    operation: "inspect guarded Ollama during shutdown",
                    source,
                })?
                .is_some();
        }
        if !ollama_process_group_exists(process_group)? {
            if !child_exited {
                ollama.wait().map_err(|source| OllamaSidecarError::Io {
                    operation: "reap guarded Ollama",
                    source,
                })?;
            }
            return Ok(());
        }
        if Instant::now() >= deadline {
            signal_ollama_process_group(process_group, Signal::KILL)?;
            ollama.wait().map_err(|source| OllamaSidecarError::Io {
                operation: "reap hard-terminated Ollama",
                source,
            })?;
            return Ok(());
        }
        thread::sleep(HEALTH_RETRY_DELAY);
    }
}

#[cfg(unix)]
fn signal_ollama_process_group(
    process_group: Pid,
    signal: Signal,
) -> Result<(), OllamaSidecarError> {
    match kill_process_group(process_group, signal) {
        Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
        Err(source) => Err(OllamaSidecarError::Io {
            operation: "terminate guarded Ollama's process group",
            source: source.into(),
        }),
    }
}

#[cfg(unix)]
fn ollama_process_group_exists(process_group: Pid) -> Result<bool, OllamaSidecarError> {
    match test_kill_process_group(process_group) {
        Ok(()) => Ok(true),
        // macOS may report EPERM after the owned group has disappeared and the
        // numeric group ID now resolves only to unsignalable system state. No
        // process inherited from our child can have different credentials.
        Err(rustix::io::Errno::SRCH | rustix::io::Errno::PERM) => Ok(false),
        Err(source) => Err(OllamaSidecarError::Io {
            operation: "inspect guarded Ollama's process group",
            source: source.into(),
        }),
    }
}

#[cfg(not(unix))]
fn terminate_guarded_ollama(ollama: &mut Child) -> Result<(), OllamaSidecarError> {
    ollama.kill().map_err(|source| OllamaSidecarError::Io {
        operation: "terminate guarded Ollama",
        source,
    })?;
    ollama.wait().map_err(|source| OllamaSidecarError::Io {
        operation: "reap guarded Ollama",
        source,
    })?;
    Ok(())
}

fn await_healthy(
    state: &mut SupervisorState,
    endpoint: OllamaSidecarEndpoint,
) -> Result<(), OllamaSidecarError> {
    let client = BlockingClient::build(
        Client::builder()
            .connect_timeout(HEALTH_REQUEST_TIMEOUT)
            .timeout(HEALTH_REQUEST_TIMEOUT),
    )
    .map_err(|()| OllamaSidecarError::HealthCheckUnavailable)?;
    let url = format!("http://{}/api/version", endpoint.address);
    let deadline = Instant::now() + STARTUP_DEADLINE;
    loop {
        refresh_state(state)?;
        if state.process.is_none() {
            return Err(OllamaSidecarError::ExitedDuringStartup);
        }
        if let Ok(response) = client.get(&url).send()
            && response.status().is_success()
        {
            let version = response
                .json::<VersionResponse>()
                .map_err(|_| OllamaSidecarError::IncompatibleVersion)?;
            if version.version == PINNED_OLLAMA_VERSION {
                return Ok(());
            }
            return Err(OllamaSidecarError::IncompatibleVersion);
        }
        if Instant::now() >= deadline {
            return Err(OllamaSidecarError::StartupTimeout);
        }
        thread::sleep(HEALTH_RETRY_DELAY);
    }
}

#[derive(Deserialize)]
struct VersionResponse {
    version: String,
}

fn canonical_file(path: &Path) -> Result<PathBuf, OllamaSidecarError> {
    let canonical = path
        .canonicalize()
        .map_err(|source| OllamaSidecarError::Io {
            operation: "resolve the bundled Ollama executable",
            source,
        })?;
    if !canonical.is_file() {
        return Err(OllamaSidecarError::InvalidExecutable);
    }
    Ok(canonical)
}

fn prepare_private_directory(path: &Path) -> Result<PathBuf, OllamaSidecarError> {
    fs::create_dir_all(path).map_err(|source| OllamaSidecarError::Io {
        operation: "create the private Ollama model store",
        source,
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|source| {
            OllamaSidecarError::Io {
                operation: "restrict the private Ollama model store",
                source,
            }
        })?;
    }
    let canonical = path
        .canonicalize()
        .map_err(|source| OllamaSidecarError::Io {
            operation: "resolve the private Ollama model store",
            source,
        })?;
    if !canonical.is_dir() {
        return Err(OllamaSidecarError::InvalidModelsDirectory);
    }
    Ok(canonical)
}

fn refresh_state(state: &mut SupervisorState) -> Result<(), OllamaSidecarError> {
    let Some(process) = state.process.as_mut() else {
        return Ok(());
    };
    if let Some(exit) = process
        .try_wait()
        .map_err(|source| OllamaSidecarError::Io {
            operation: "inspect bundled Ollama",
            source,
        })?
    {
        state.process = None;
        state.graceful_shutdown = false;
        state.endpoint = None;
        state.last_exit = Some(exit);
    }
    Ok(())
}

fn terminate(state: &mut SupervisorState) -> Result<(), OllamaSidecarError> {
    let Some(mut process) = state.process.take() else {
        state.endpoint = None;
        state.last_exit = None;
        return Ok(());
    };
    if process
        .try_wait()
        .map_err(|source| OllamaSidecarError::Io {
            operation: "inspect bundled Ollama before shutdown",
            source,
        })?
        .is_none()
    {
        if state.graceful_shutdown {
            drop(process.stdin.take());
            let deadline = Instant::now() + GRACEFUL_SHUTDOWN_TIMEOUT;
            while process
                .try_wait()
                .map_err(|source| OllamaSidecarError::Io {
                    operation: "inspect bundled Ollama during graceful shutdown",
                    source,
                })?
                .is_none()
            {
                if Instant::now() >= deadline {
                    process.kill().map_err(|source| OllamaSidecarError::Io {
                        operation: "hard terminate bundled Ollama guardian",
                        source,
                    })?;
                    break;
                }
                thread::sleep(HEALTH_RETRY_DELAY);
            }
        } else {
            process.kill().map_err(|source| OllamaSidecarError::Io {
                operation: "terminate bundled Ollama",
                source,
            })?;
        }
    }
    process.wait().map_err(|source| OllamaSidecarError::Io {
        operation: "reap bundled Ollama",
        source,
    })?;
    state.endpoint = None;
    state.graceful_shutdown = false;
    state.last_exit = None;
    Ok(())
}

/// Failure while launching or supervising the bundled Ollama runtime.
#[derive(Debug)]
pub enum OllamaSidecarError {
    /// The pinned executable is missing or is not a regular file.
    InvalidExecutable,
    /// The private model-store path is not a directory.
    InvalidModelsDirectory,
    /// A child is already active.
    AlreadyRunning,
    /// The operating system returned a non-loopback endpoint.
    NonLoopbackEndpoint,
    /// The supervisor lock is poisoned.
    SupervisorUnavailable,
    /// The bounded health client could not be created.
    HealthCheckUnavailable,
    /// The child did not become healthy before the deadline.
    StartupTimeout,
    /// The child exited before passing its health check.
    ExitedDuringStartup,
    /// The child did not report the exact pinned Ollama version.
    IncompatibleVersion,
    /// A sanitized operating-system failure.
    Io {
        /// Stable operation description containing no private path.
        operation: &'static str,
        /// Underlying I/O error.
        source: std::io::Error,
    },
}

impl fmt::Display for OllamaSidecarError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidExecutable => {
                formatter.write_str("the bundled Ollama executable is invalid")
            }
            Self::InvalidModelsDirectory => {
                formatter.write_str("the private Ollama model store is invalid")
            }
            Self::AlreadyRunning => formatter.write_str("bundled Ollama is already running"),
            Self::NonLoopbackEndpoint => {
                formatter.write_str("Ollama did not receive a loopback endpoint")
            }
            Self::SupervisorUnavailable => {
                formatter.write_str("the Ollama supervisor is unavailable")
            }
            Self::HealthCheckUnavailable => {
                formatter.write_str("the Ollama health check is unavailable")
            }
            Self::StartupTimeout => formatter.write_str("bundled Ollama did not start in time"),
            Self::ExitedDuringStartup => {
                formatter.write_str("bundled Ollama exited during startup")
            }
            Self::IncompatibleVersion => {
                formatter.write_str("the bundled Ollama version is incompatible")
            }
            Self::Io { operation, .. } => write!(formatter, "could not {operation}"),
        }
    }
}

impl Error for OllamaSidecarError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsStr, fs, io, path::PathBuf};

    use tempfile::tempdir;

    use super::*;
    use crate::{GenerationEvent, GenerationMetrics, QwenStreamDecoder};

    #[test]
    fn launch_is_private_loopback_bounded_and_log_free() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let executable = directory.path().join("ollama");
        let models = directory.path().join("private-models");
        fs::write(&executable, b"test executable")?;
        let prepared = PreparedLaunch::new(&OllamaSidecarConfig::new(&executable, &models))?;
        let environment = prepared.command.get_envs().collect::<Vec<_>>();
        let canonical_models = models.canonicalize()?;

        assert_eq!(*prepared.endpoint.address.ip(), Ipv4Addr::LOCALHOST);
        assert_eq!(
            prepared.command.get_args().collect::<Vec<_>>(),
            [OsStr::new("serve")]
        );
        assert!(environment.iter().any(|(key, value)| {
            *key == OsStr::new("OLLAMA_HOST")
                && *value == Some(OsStr::new(&prepared.endpoint.address.to_string()))
        }));
        assert!(environment.iter().any(|(key, value)| {
            *key == OsStr::new("OLLAMA_MODELS") && *value == Some(canonical_models.as_os_str())
        }));
        assert!(environment.iter().any(|(key, value)| {
            *key == OsStr::new("HOME") && *value == Some(canonical_models.as_os_str())
        }));
        assert!(environment.iter().any(|(key, value)| {
            *key == OsStr::new("OLLAMA_NO_CLOUD") && *value == Some(OsStr::new("1"))
        }));
        assert!(environment.iter().any(|(key, value)| {
            *key == OsStr::new("OLLAMA_NOPRUNE") && *value == Some(OsStr::new("1"))
        }));
        assert!(environment.iter().any(|(key, value)| {
            *key == OsStr::new("OLLAMA_KEEP_ALIVE") && *value == Some(OsStr::new("5m"))
        }));
        assert!(environment.iter().any(|(key, value)| {
            *key == OsStr::new("OLLAMA_MAX_LOADED_MODELS") && *value == Some(OsStr::new("1"))
        }));
        assert!(environment.iter().any(|(key, value)| {
            *key == OsStr::new("OLLAMA_NUM_PARALLEL") && *value == Some(OsStr::new("1"))
        }));
        assert_eq!(fs::metadata(&models)?.permissions().mode() & 0o777, 0o700);
        Ok(())
    }

    #[test]
    fn guarded_launch_routes_through_the_owner_process() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let executable = directory.path().join("ollama");
        let guardian = directory.path().join("zeta-desktop");
        let models = directory.path().join("private-models");
        fs::write(&executable, b"test executable")?;
        fs::write(&guardian, b"test guardian")?;

        let prepared = PreparedLaunch::new(
            &OllamaSidecarConfig::new(&executable, &models).with_guardian_executable(&guardian),
        )?;

        assert!(prepared.graceful_shutdown);
        assert_eq!(prepared.command.get_program(), guardian.canonicalize()?);
        assert_eq!(
            prepared.command.get_args().collect::<Vec<_>>(),
            [
                OsStr::new(OLLAMA_GUARDIAN_ARGUMENT),
                executable.canonicalize()?.as_os_str()
            ]
        );
        #[cfg(debug_assertions)]
        {
            let trace_directory = directory.path().join("trace");
            let traced = PreparedLaunch::new(
                &OllamaSidecarConfig::new(&executable, &models)
                    .with_guardian_executable(&guardian)
                    .with_cache_telemetry_directory(&trace_directory),
            )?;
            let environment = traced.command.get_envs().collect::<Vec<_>>();
            assert!(environment.iter().any(|(key, value)| {
                *key == OsStr::new("ZETA_DEV_OLLAMA_CACHE_TELEMETRY_DIRECTORY")
                    && *value == Some(trace_directory.as_os_str())
            }));
            assert!(environment.iter().any(|(key, value)| {
                *key == OsStr::new("OLLAMA_DEBUG") && *value == Some(OsStr::new("1"))
            }));
        }
        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn guardian_stops_and_reaps_ollama_when_ownership_closes() -> Result<(), Box<dyn Error>> {
        run_ollama_guardian_with_reader(
            Path::new("/usr/bin/yes"),
            std::io::Cursor::new(Vec::<u8>::new()),
        )?;
        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn guardian_terminates_descendants_in_ollamas_owned_process_group() -> Result<(), Box<dyn Error>>
    {
        struct CloseAfterPidIsRecorded {
            path: PathBuf,
            deadline: Instant,
        }

        impl Read for CloseAfterPidIsRecorded {
            fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
                while !self.path.exists() {
                    if Instant::now() >= self.deadline {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "descendant PID was not recorded",
                        ));
                    }
                    thread::sleep(HEALTH_RETRY_DELAY);
                }
                Ok(0)
            }
        }

        let directory = tempdir()?;
        let executable = directory.path().join("ollama-test");
        let descendant_pid_path = directory.path().join("runner.pid");
        fs::write(
            &executable,
            format!(
                "#!/bin/sh\nsleep 30 &\necho $! > '{}'\nwait\n",
                descendant_pid_path.display()
            ),
        )?;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))?;

        run_ollama_guardian_with_reader(
            &executable,
            CloseAfterPidIsRecorded {
                path: descendant_pid_path.clone(),
                deadline: Instant::now() + Duration::from_secs(2),
            },
        )?;

        let raw_pid = fs::read_to_string(descendant_pid_path)?
            .trim()
            .parse::<i32>()?;
        let descendant_pid = Pid::from_raw(raw_pid).ok_or("invalid descendant PID")?;
        let deadline = Instant::now() + Duration::from_secs(2);
        while rustix::process::test_kill_process(descendant_pid).is_ok()
            && Instant::now() < deadline
        {
            thread::sleep(HEALTH_RETRY_DELAY);
        }
        assert_eq!(
            rustix::process::test_kill_process(descendant_pid),
            Err(rustix::io::Errno::SRCH)
        );
        Ok(())
    }

    #[test]
    fn duplicate_launch_is_rejected_without_replacing_the_child() -> Result<(), Box<dyn Error>> {
        let supervisor = OllamaSidecar::new();
        let mut state = supervisor.lock_state()?;
        state.process = Some(Command::new("/usr/bin/yes").stdout(Stdio::null()).spawn()?);
        drop(state);

        let directory = tempdir()?;
        let config = OllamaSidecarConfig::new("/usr/bin/yes", directory.path().join("models"));
        assert!(matches!(
            supervisor.start(&config),
            Err(OllamaSidecarError::AlreadyRunning)
        ));
        supervisor.stop()?;
        Ok(())
    }

    #[test]
    #[ignore = "requires the locally installed pinned Ollama application"]
    #[cfg(target_os = "macos")]
    fn live_pinned_ollama_starts_on_private_ephemeral_endpoint() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let executable = Path::new("/Applications/Ollama.app/Contents/Resources/ollama");
        let supervisor = OllamaSidecar::new();
        let config = OllamaSidecarConfig::new(executable, directory.path().join("models"));
        let endpoint = supervisor.start(&config)?;

        assert_ne!(endpoint.address().port(), 11_434);
        assert_eq!(supervisor.status()?, OllamaSidecarStatus::Running);
        {
            let mut state = supervisor.lock_state()?;
            state
                .process
                .as_mut()
                .ok_or(OllamaSidecarError::ExitedDuringStartup)?
                .kill()?;
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while supervisor.status()? == OllamaSidecarStatus::Running && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(
            supervisor.status()?,
            OllamaSidecarStatus::Exited { .. }
        ));

        let restarted = supervisor.start(&config)?;
        assert_ne!(restarted.address().port(), 0);
        assert_eq!(supervisor.status()?, OllamaSidecarStatus::Running);
        supervisor.stop()?;
        assert_eq!(supervisor.status()?, OllamaSidecarStatus::Stopped);
        Ok(())
    }

    #[test]
    #[ignore = "requires an isolated store containing the pinned Qwen bundle"]
    #[cfg(target_os = "macos")]
    fn live_bundled_runtime_serves_qwen_from_private_store() -> Result<(), Box<dyn Error>> {
        let models = std::env::var_os("ZETA_LIVE_OLLAMA_MODELS")
            .ok_or("ZETA_LIVE_OLLAMA_MODELS is required")?;
        let executable =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src-tauri/ollama-runtime/ollama");
        let supervisor = OllamaSidecar::new();
        let endpoint = supervisor.start(
            &OllamaSidecarConfig::new(&executable, models).with_guardian_executable(
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/zeta-desktop"),
            ),
        )?;
        let provider = crate::OllamaProvider::new(crate::OllamaProviderConfig::loopback(
            endpoint.address().into(),
        )?)?;
        let catalogue = crate::bundled_catalogue()?;
        let qwen = catalogue
            .models()
            .iter()
            .find(|model| model.backend() == crate::InferenceBackend::Ollama)
            .ok_or("Qwen catalogue profile is missing")?;
        let verified = provider.verify_model(qwen)?;
        let request = crate::OllamaChatRequest::new(
            &verified,
            vec![crate::OllamaChatMessage::new(
                "user",
                "Reply with exactly OK.",
            )?],
            false,
        );
        let mut content = String::new();
        let mut completed = false;
        provider.stream_chat(&request, &crate::DownloadCancellation::new(), |chunk| {
            completed |= chunk.done;
            if let Some(message) = chunk.message {
                content.push_str(&message.content);
            }
        })?;

        assert!(completed);
        assert!(!content.trim().is_empty());
        supervisor.stop()?;
        Ok(())
    }

    #[test]
    #[ignore = "requires an isolated store containing the pinned Qwen bundle"]
    #[cfg(target_os = "macos")]
    fn live_bundled_qwen_reuses_an_identical_prompt_prefix() -> Result<(), Box<dyn Error>> {
        let models = std::env::var_os("ZETA_LIVE_OLLAMA_MODELS")
            .ok_or("ZETA_LIVE_OLLAMA_MODELS is required")?;
        let executable =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src-tauri/ollama-runtime/ollama");
        let supervisor = OllamaSidecar::new();
        let endpoint = supervisor.start(
            &OllamaSidecarConfig::new(&executable, models).with_guardian_executable(
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/zeta-desktop"),
            ),
        )?;
        let provider = crate::OllamaProvider::new(crate::OllamaProviderConfig::loopback(
            endpoint.address().into(),
        )?)?;
        let catalogue = crate::bundled_catalogue()?;
        let qwen = catalogue
            .models()
            .iter()
            .find(|model| model.backend() == crate::InferenceBackend::Ollama)
            .ok_or("Qwen catalogue profile is missing")?;
        let verified = provider.verify_model(qwen)?;
        let stable = "stable-prefix-probe ".repeat(512);
        assert_plain_prefix_reuse(&provider, &verified, &stable)?;
        assert_tool_prefix_reuse(&provider, &verified, &stable)?;
        supervisor.stop()?;
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn run_qwen_cache_probe(
        provider: &crate::OllamaProvider,
        request: &crate::OllamaChatRequest,
        generation_id: &str,
    ) -> Result<GenerationMetrics, Box<dyn Error>> {
        let _ = provider.render_chat_prompt(request)?;
        let mut decoder = QwenStreamDecoder::new(generation_id)?;
        let mut metrics = None;
        provider.stream_chat_events(
            request,
            &crate::DownloadCancellation::new(),
            &mut decoder,
            |event| {
                if let GenerationEvent::Metrics { metrics: observed } = event {
                    metrics = Some(observed);
                }
            },
        )?;
        metrics.ok_or_else(|| "Qwen did not report generation metrics".into())
    }

    #[cfg(target_os = "macos")]
    fn assert_plain_prefix_reuse(
        provider: &crate::OllamaProvider,
        verified: &crate::VerifiedOllamaModel,
        stable: &str,
    ) -> Result<(), Box<dyn Error>> {
        let request = crate::PromptRenderRequest {
            messages: vec![crate::PromptMessage::User {
                content: vec![crate::PromptContent::Text(format!(
                    "{stable}\nReply with exactly OK."
                ))],
            }],
            tools: Vec::new(),
            historical_tool_names: Vec::new(),
            current_date: "2026-08-31".to_owned(),
            reasoning_strength: crate::ReasoningStrength::Low,
            add_generation_prompt: true,
        };
        let request =
            crate::QwenAdapter.render_with_context(verified, &request, &[], 32_768, 32)?;

        let cold = run_qwen_cache_probe(provider, &request, "cache-probe-cold")?;
        let repeated = run_qwen_cache_probe(provider, &request, "cache-probe-repeat")?;
        let suffix_request = crate::PromptRenderRequest {
            messages: vec![crate::PromptMessage::User {
                content: vec![crate::PromptContent::Text(format!(
                    "{stable}\nOne appended fact. Reply with exactly OK."
                ))],
            }],
            tools: Vec::new(),
            historical_tool_names: Vec::new(),
            current_date: "2026-08-31".to_owned(),
            reasoning_strength: crate::ReasoningStrength::Low,
            add_generation_prompt: true,
        };
        let suffix_request =
            crate::QwenAdapter.render_with_context(verified, &suffix_request, &[], 32_768, 32)?;
        let suffix = run_qwen_cache_probe(provider, &suffix_request, "cache-probe-suffix")?;
        let cold_tokens = cold.prompt_tokens.ok_or("cold prompt count missing")?;
        let repeated_tokens = repeated
            .prompt_tokens
            .ok_or("repeated prompt count missing")?;
        let cold_ns = cold
            .prompt_evaluation_ns
            .ok_or("cold prompt duration missing")?;
        let repeated_ns = repeated
            .prompt_evaluation_ns
            .ok_or("repeated prompt duration missing")?;
        let suffix_tokens = suffix.prompt_tokens.ok_or("suffix prompt count missing")?;
        let suffix_ns = suffix
            .prompt_evaluation_ns
            .ok_or("suffix prompt duration missing")?;

        assert!(
            repeated_ns.saturating_mul(4) < cold_ns && suffix_ns.saturating_mul(2) < cold_ns,
            "Qwen prompt prefix was not materially reused: cold_tokens={cold_tokens}, repeated_tokens={repeated_tokens}, suffix_tokens={suffix_tokens}, cold_ns={cold_ns}, repeated_ns={repeated_ns}, suffix_ns={suffix_ns}"
        );
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn assert_tool_prefix_reuse(
        provider: &crate::OllamaProvider,
        verified: &crate::VerifiedOllamaModel,
        stable: &str,
    ) -> Result<(), Box<dyn Error>> {
        let tool = crate::ollama::OllamaToolDefinition::new(
            "cache_probe".to_owned(),
            "Return the supplied bounded value.".to_owned(),
            serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {"value": {"type": "string"}},
                "required": ["value"]
            }),
        );
        let tool_messages = vec![
            crate::OllamaChatMessage::new("system", stable)?,
            crate::OllamaChatMessage::new("user", "Call cache_probe with value OK.")?,
        ];
        let tool_request = crate::OllamaChatRequest::native(
            verified,
            tool_messages.clone(),
            vec![tool.clone()],
            crate::ReasoningStrength::Low,
        )
        .with_context(32_768, 32);
        let tool_cold = run_qwen_cache_probe(provider, &tool_request, "cache-probe-tool-cold")?;
        let mut continuation_messages = tool_messages;
        continuation_messages.push(crate::OllamaChatMessage::native(
            "assistant",
            String::new(),
            None,
            Vec::new(),
            vec![crate::ollama::OllamaToolCall::new(
                "probe-call".to_owned(),
                "cache_probe".to_owned(),
                serde_json::json!({"value": "OK"}),
            )],
            None,
            None,
        ));
        continuation_messages.push(crate::OllamaChatMessage::native(
            "tool",
            "{\"accepted\":true}".to_owned(),
            None,
            Vec::new(),
            Vec::new(),
            Some("cache_probe".to_owned()),
            Some("probe-call".to_owned()),
        ));
        let continuation_request = crate::OllamaChatRequest::native(
            verified,
            continuation_messages,
            vec![tool],
            crate::ReasoningStrength::Low,
        )
        .with_context(32_768, 32);
        let continuation = run_qwen_cache_probe(
            provider,
            &continuation_request,
            "cache-probe-tool-continuation",
        )?;
        let tool_cold_ns = tool_cold
            .prompt_evaluation_ns
            .ok_or("tool cold prompt duration missing")?;
        let continuation_ns = continuation
            .prompt_evaluation_ns
            .ok_or("tool continuation prompt duration missing")?;
        assert!(
            continuation_ns.saturating_mul(2) < tool_cold_ns,
            "Qwen tool continuation did not reuse its stable prefix: tool_cold_ns={tool_cold_ns}, continuation_ns={continuation_ns}"
        );
        Ok(())
    }

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
}
