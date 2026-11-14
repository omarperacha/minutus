mod diagnostics;
mod pipes;
use crate::{CompilationError, CompilationInput, ReproducedArtifact, native_identity};
use diagnostics::stage as diagnostic_stage;
#[cfg(test)]
use diagnostics::{early_exit as diagnostic_early_exit, enabled as diagnostics_enabled};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[cfg(test)]
use std::process::{Command, Stdio};
use std::{
    fs,
    io::{Read, Write},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

#[cfg(test)]
mod containment_tests;
#[cfg(test)]
mod immutable_tests;
#[cfg(test)]
mod tests;

const NAME: &str = "minutus-solidity-compiler";
const WALL: Duration = Duration::from_mins(1);
#[cfg(test)]
const MEMORY: u64 = 2 * 1024 * 1024 * 1024;
#[cfg(test)]
const SANDBOX: &str = r#"(version 1)
(deny default)
(allow process-exec (literal (param "WORKER")))
(allow file-read-metadata)
(allow file-read-data (literal "/"))
(allow file-read-data (literal (param "WORKER")) (subpath "/System/Library") (subpath "/usr/lib"))
(allow sysctl-read)
(allow process-info* (target self))
(allow signal (target self))
"#;

/// Compiler helper pinned into the loaded host image. It owns no endpoint,
/// wallet, signing API or filesystem grant. There is no raw-path constructor.
pub struct TrustedCompiler {
    path: PathBuf,
    digest: String,
    cdhash: [u8; 20],
    transport: Option<worker_transport::PackagedWorker>,
}
impl TrustedCompiler {
    /// Loads the exact packaged compiler identity from this host's immutable
    /// image and its fixed sibling path. Ordinary unpinned dev builds reject.
    ///
    /// # Errors
    /// Rejects absent, malformed, indirect or changed packaged helpers.
    pub fn packaged() -> Result<Self, CompilationError> {
        let manifest = native_identity::host_manifest()?;
        let text =
            std::str::from_utf8(&manifest).map_err(|_| CompilationError::WorkerUnavailable)?;
        let lines: Vec<_> = text.lines().collect();
        if lines.len() != 2 || lines[0] != "minutus-solidity-compiler-v1" {
            return Err(CompilationError::WorkerUnavailable);
        }
        let fields: Vec<_> = lines[1].split(' ').collect();
        if fields.len() != 3 || fields[0] != NAME || fields[1].len() != 64 || fields[2].len() != 40
        {
            return Err(CompilationError::WorkerUnavailable);
        }
        let mut cdhash = [0; 20];
        for (place, pair) in cdhash.iter_mut().zip(fields[2].as_bytes().chunks_exact(2)) {
            let s = std::str::from_utf8(pair).map_err(|_| CompilationError::WorkerUnavailable)?;
            *place = u8::from_str_radix(s, 16).map_err(|_| CompilationError::WorkerUnavailable)?;
        }
        let executable =
            std::env::current_exe().map_err(|_| CompilationError::WorkerUnavailable)?;
        let directory = executable
            .parent()
            .ok_or(CompilationError::WorkerUnavailable)?;
        let transport =
            worker_transport::PackagedWorker::packaged(worker_transport::Role::Compiler)
                .map_err(transport_error)?;
        if transport.worker_sha256() != fields[1] || transport.worker_cdhash() != fields[2] {
            return Err(CompilationError::WorkerUnavailable);
        }
        let worker = Self {
            path: directory.join(NAME),
            digest: fields[1].into(),
            cdhash,
            transport: Some(transport),
        };
        worker.verify_file()?;
        Ok(worker)
    }
    fn verify_file(&self) -> Result<(), CompilationError> {
        if self.transport.as_ref().is_some_and(|transport| {
            transport.worker_cdhash() != crate::bytecode::encode(&self.cdhash)
        }) {
            return Err(CompilationError::WorkerUnavailable);
        }
        let file = rustix::fs::open(
            &self.path,
            rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .map_err(|_| CompilationError::WorkerUnavailable)?;
        let file = fs::File::from(file);
        let metadata = file
            .metadata()
            .map_err(|_| CompilationError::WorkerUnavailable)?;
        if !metadata.is_file() || metadata.len() > 16 * 1024 * 1024 {
            return Err(CompilationError::WorkerUnavailable);
        }
        let mut bytes = Vec::new();
        file.take(16 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| CompilationError::WorkerUnavailable)?;
        if bytes.len() > 16 * 1024 * 1024 {
            return Err(CompilationError::WorkerUnavailable);
        }
        if format!("{:x}", Sha256::digest(&bytes)) != self.digest {
            return Err(CompilationError::WorkerUnavailable);
        }
        Ok(())
    }
    /// Reproduces self-contained sources with official checksum-pinned compiler
    /// bytes. Compiler/source data enter only after running-image attestation.
    ///
    /// # Errors
    /// Rejects changed compiler/helper identity, cancellation, resource bounds,
    /// source errors and unsupported/inconsistent artifacts.
    pub fn reproduce(
        &self,
        input: &CompilationInput,
        compiler: &[u8],
        cancelled: &AtomicBool,
    ) -> Result<ReproducedArtifact, CompilationError> {
        decode_response(input, &self.compile_response(input, compiler, cancelled)?)
    }
    /// Reproduces a runtime template and binds only compiler-enumerated immutable
    /// words to the supplied runtime. The caller must independently prove those
    /// current code bytes; this method supplies no deployment/proof authority.
    /// Existing link references remain unsupported.
    ///
    /// # Errors
    /// Rejects all ordinary reproduction failures, unresolved immutable types,
    /// inconsistent occurrences, noncanonical values or any unmatched code byte.
    pub fn reproduce_for_runtime(
        &self,
        input: &CompilationInput,
        compiler: &[u8],
        runtime: &[u8],
        cancelled: &AtomicBool,
    ) -> Result<ReproducedArtifact, CompilationError> {
        if runtime.is_empty() || runtime.len() > 24_576 {
            return Err(CompilationError::Limit);
        }
        let response = self.compile_response(input, compiler, cancelled)?;
        ReproducedArtifact::for_runtime(input, &decode_output(input, &response)?, runtime)
    }
    fn compile_response(
        &self,
        input: &CompilationInput,
        compiler: &[u8],
        cancelled: &AtomicBool,
    ) -> Result<Vec<u8>, CompilationError> {
        input.build.verify(compiler)?;
        let compiler =
            std::str::from_utf8(compiler).map_err(|_| CompilationError::CompilerMismatch)?;
        let standard =
            serde_json::to_string(&input.standard).map_err(|_| CompilationError::InvalidInput)?;
        let bytes = serde_json::to_vec(&Request {
            compiler,
            version: input.build.version(),
            input: &standard,
        })
        .map_err(|_| CompilationError::InvalidInput)?;
        if bytes.len() > 64 * 1024 * 1024 {
            return Err(CompilationError::Limit);
        }
        if COMPILER_POISONED.load(Ordering::Acquire) {
            return Err(CompilationError::UnconfirmedTermination);
        }
        let transport = self
            .transport
            .as_ref()
            .ok_or(CompilationError::WorkerUnavailable)?;
        let started = Instant::now();
        let mut process = transport
            .start_with_deadline(WALL, cancelled)
            .map_err(transport_error)?;
        diagnostic_stage("transport_ready");
        let stop = AtomicBool::new(false);
        let pipes = prepare_pipes(&mut process, started, cancelled, &stop);
        let (mut stdin, stdout) = match pipes {
            Ok(pipes) => pipes,
            Err(error) => {
                return match process.cancel_and_reap() {
                    Ok(_) => Err(error),
                    Err(error) => Err(transport_error(error)),
                };
            }
        };
        thread::scope(|scope| {
            let writer = scope.spawn(move || stdin.write_all(&bytes));
            let (send, receive) = mpsc::sync_channel(1);
            let reader = scope.spawn(move || {
                let _ = send.send(read_output(stdout));
            });
            let result = loop {
                if let Err(error) = check(started, cancelled) {
                    break Err(error);
                }
                if let Err(error) = process.check() {
                    break Err(transport_error(error));
                }
                match receive.recv_timeout(Duration::from_millis(20)) {
                    Ok(result) => break result,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(_) => break Err(CompilationError::CompilationFailed),
                }
            };
            let reaped = if result.is_err() {
                process.cancel_and_reap()
            } else {
                process.finish_with_deadline(WALL.saturating_sub(started.elapsed()), cancelled)
            };
            if let Err(error) = reaped.as_ref() {
                let _ = process.cancel_and_reap();
                if *error == worker_transport::Error::UnconfirmedTermination {
                    COMPILER_POISONED.store(true, Ordering::Release);
                }
            }
            stop.store(true, Ordering::Release);
            let write = writer
                .join()
                .map_err(|_| CompilationError::CompilationFailed)
                .and_then(|v| v.map_err(|_| CompilationError::CompilationFailed));
            let _ = reader.join();
            let reaped = reaped.map_err(transport_error)?;
            let output = result?;
            write?;
            if !reaped.success() {
                return Err(CompilationError::CompilationFailed);
            }
            check(started, cancelled)?;
            Ok(output)
        })
    }
    #[cfg(test)]
    fn launch(&self, started: Instant, cancelled: &AtomicBool) -> Result<Reaped, CompilationError> {
        self.verify_file()?;
        check(started, cancelled)?;
        let child = Command::new("/usr/bin/sandbox-exec")
            .args(["-p", SANDBOX, "-D"])
            .arg(format!(
                "WORKER={}",
                self.path
                    .to_str()
                    .ok_or(CompilationError::WorkerUnavailable)?
            ))
            .arg(&self.path)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(if diagnostics_enabled() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .spawn()
            .map_err(|_| CompilationError::WorkerUnavailable)?;
        let mut child = Reaped(child);
        diagnostic_stage("spawn_succeeded");
        // Never reap before checking identity: our PID ownership survives early exit.
        loop {
            check(started, cancelled)?;
            let identity = native_identity::child_hash(child.0.id());
            if identity.is_ok_and(|hash| hash == self.cdhash) {
                if native_identity::child_exited(child.0.id())? {
                    diagnostic_stage("child_exited_during_attestation");
                    diagnostic_early_exit(&mut child.0);
                    return Err(CompilationError::WorkerUnavailable);
                }
                break;
            }
            match native_identity::child_exited(child.0.id()) {
                Ok(true) => {
                    diagnostic_stage("child_exited_before_attestation");
                    diagnostic_early_exit(&mut child.0);
                    return Err(CompilationError::WorkerUnavailable);
                }
                Err(error) => {
                    diagnostic_stage("identity_query_failed_exitcheck");
                    return Err(error);
                }
                Ok(false) => {}
            }
            if started.elapsed() > Duration::from_secs(2) {
                diagnostic_stage(if identity.is_err() {
                    "attestation_deadline_identity_unavailable"
                } else {
                    "attestation_deadline_hash_mismatch"
                });
                return Err(CompilationError::WorkerUnavailable);
            }
            thread::sleep(Duration::from_millis(5));
        }
        diagnostic_stage("running_image_attested");
        Ok(child)
    }
}
fn decode_response(
    input: &CompilationInput,
    bytes: &[u8],
) -> Result<ReproducedArtifact, CompilationError> {
    ReproducedArtifact::from_compiler(input, &decode_output(input, bytes)?)
}
fn decode_output(
    input: &CompilationInput,
    bytes: &[u8],
) -> Result<serde_json::Value, CompilationError> {
    let response: Response =
        serde_json::from_slice(bytes).map_err(|_| CompilationError::CompilationFailed)?;
    if response.version != format!("{}.Emscripten.clang", input.build.version()) {
        return Err(CompilationError::CompilerMismatch);
    }
    let output =
        serde_json::from_str(&response.output).map_err(|_| CompilationError::CompilationFailed)?;
    Ok(output)
}
fn read_output(stdout: impl Read) -> Result<Vec<u8>, CompilationError> {
    let mut out = Vec::new();
    stdout
        .take(32 * 1024 * 1024 + 1)
        .read_to_end(&mut out)
        .map_err(|_| CompilationError::CompilationFailed)?;
    if out.len() > 32 * 1024 * 1024 {
        return Err(CompilationError::Limit);
    }
    Ok(out)
}
#[derive(Serialize)]
struct Request<'a> {
    compiler: &'a str,
    version: &'a str,
    input: &'a str,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    version: String,
    output: String,
}
#[cfg(test)]
struct Reaped(std::process::Child);
#[cfg(test)]
impl Drop for Reaped {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn check(started: Instant, cancelled: &AtomicBool) -> Result<(), CompilationError> {
    if cancelled.load(Ordering::Acquire) {
        return Err(CompilationError::Cancelled);
    }
    if started.elapsed() >= WALL {
        return Err(CompilationError::Limit);
    }
    Ok(())
}
impl std::fmt::Debug for TrustedCompiler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrustedCompiler").finish_non_exhaustive()
    }
}
#[cfg(test)]
fn memory(child: &std::process::Child) -> Result<bool, CompilationError> {
    match native_identity::physical_bytes(child.id()) {
        Ok(bytes) if bytes <= MEMORY => Ok(true),
        Ok(_) => Err(CompilationError::Limit),
        Err(_) if native_identity::child_exited(child.id())? => Ok(false),
        Err(error) => Err(error),
    }
}
#[cfg(test)]
fn await_exit(
    child: &mut std::process::Child,
    started: Instant,
    cancelled: &AtomicBool,
) -> Result<std::process::ExitStatus, CompilationError> {
    loop {
        check(started, cancelled)?;
        memory(child)?;
        if let Some(status) = child
            .try_wait()
            .map_err(|_| CompilationError::WorkerUnavailable)?
        {
            return Ok(status);
        }
        thread::sleep(Duration::from_millis(10));
    }
}

static COMPILER_POISONED: AtomicBool = AtomicBool::new(false);
fn transport_error(error: worker_transport::Error) -> CompilationError {
    match error {
        worker_transport::Error::Cancelled => CompilationError::Cancelled,
        worker_transport::Error::Limit => CompilationError::Limit,
        worker_transport::Error::UnconfirmedTermination => {
            COMPILER_POISONED.store(true, Ordering::Release);
            CompilationError::UnconfirmedTermination
        }
        worker_transport::Error::Unavailable | worker_transport::Error::Protocol => {
            CompilationError::WorkerUnavailable
        }
    }
}

fn prepare_pipes<'a>(
    process: &mut worker_transport::ReadyChannel,
    started: Instant,
    cancelled: &'a AtomicBool,
    stop: &'a AtomicBool,
) -> Result<(pipes::TimedPipe<'a>, pipes::TimedPipe<'a>), CompilationError> {
    let input = process.take_stdin().map_err(transport_error)?;
    let output = process.take_stdout().map_err(transport_error)?;
    Ok((
        pipes::TimedPipe::new(input, started, cancelled, stop)?,
        pipes::TimedPipe::new(output, started, cancelled, stop)?,
    ))
}
