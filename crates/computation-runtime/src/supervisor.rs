use std::{
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use crate::{
    ComputationRequest, ComputationResponse, RunnerStatus,
    protocol::{
        LAUNCH_TOKEN_BYTES, LAUNCH_TOKEN_ENV, RequestEnvelope, decode_response, encode_request,
    },
    resource_policy::{
        CPU_TIME_LIMIT, PROCESS_MEMORY_LIMIT_BYTES, WALL_TIME_LIMIT, committed_to_exit,
        process_usage,
    },
};

const POLL_INTERVAL: Duration = Duration::from_millis(10);
const BROKER_CPU_LIMIT_EXIT: i32 = 70;
const BROKER_MEMORY_LIMIT_EXIT: i32 = 71;
const BROKER_POLICY_UNAVAILABLE_EXIT: i32 = 72;
const BROKER_WALL_LIMIT_EXIT: i32 = 73;
const BROKER_SERVICE_FAILED_EXIT: i32 = 74;
const BROKER_UNAVAILABLE_EXIT: i32 = 75;

/// Host-owned cancellation flag for one computation launch.
#[derive(Clone, Debug, Default)]
pub struct ComputationCancellation(Arc<AtomicBool>);

impl ComputationCancellation {
    /// Creates an active cancellation flag.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Marks the associated request cancelled.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Launches one fresh application-bundled computation process per request.
#[derive(Clone, Debug)]
pub struct ComputationRunner {
    executable: PathBuf,
}

impl ComputationRunner {
    /// Creates a supervisor for one application-managed runner executable.
    #[must_use]
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
        }
    }

    /// Executes one request, then terminates and reaps its dedicated process.
    ///
    /// The child receives a cleared environment containing only a fresh private
    /// protocol token. Standard error is discarded and no user-controlled
    /// executable, argument, environment or working directory crosses this API.
    ///
    /// # Errors
    ///
    /// Returns a sanitized error for an invalid executable, failed launch,
    /// cancellation, deadline, crash, malformed protocol or mismatched response.
    pub fn execute(
        &self,
        request: &ComputationRequest,
        cancellation: &ComputationCancellation,
    ) -> Result<ComputationResponse, RunnerError> {
        self.execute_with_cancel_check(request, || cancellation.is_cancelled())
    }

    /// Executes one request while observing a shared host cancellation flag.
    ///
    /// This adapts the agent runtime's existing cancellation owner without a
    /// second thread or a computation-specific task lifecycle.
    ///
    /// # Errors
    ///
    /// Returns the same sanitized launch, protocol and process failures as
    /// [`Self::execute`].
    pub fn execute_with_host_cancellation(
        &self,
        request: &ComputationRequest,
        cancellation: &AtomicBool,
    ) -> Result<ComputationResponse, RunnerError> {
        self.execute_with_cancel_check(request, || cancellation.load(Ordering::Acquire))
    }

    fn execute_with_cancel_check(
        &self,
        request: &ComputationRequest,
        cancelled: impl Fn() -> bool,
    ) -> Result<ComputationResponse, RunnerError> {
        request
            .validate()
            .map_err(|()| RunnerError::InvalidRequest)?;
        if cancelled() {
            return Ok(cancelled_response(request.request_id()));
        }
        let executable = canonical_executable(&self.executable)?;
        if cancelled() {
            return Ok(cancelled_response(request.request_id()));
        }
        let token = LaunchToken::generate()?;
        let token_hex = token.expose_hex();
        let envelope = RequestEnvelope::new(token_hex.clone(), request.clone());
        let mut request_frame = Vec::new();
        encode_request(&envelope, &mut request_frame)?;
        if cancelled() {
            return Ok(cancelled_response(request.request_id()));
        }
        let mut child = launch(&executable, &token)?;
        let result = supervise(
            &mut child,
            request_frame,
            request.request_id(),
            token_hex,
            &cancelled,
        );
        let cleanup = terminate_and_reap(&mut child);
        match (result, cleanup) {
            (Ok(response), Ok(())) => Ok(response),
            (Err(error), _) | (Ok(_), Err(error)) => Err(error),
        }
    }
}

fn cancelled_response(request_id: &str) -> ComputationResponse {
    ComputationResponse::terminal(request_id, RunnerStatus::Cancelled, None)
}

fn supervise(
    child: &mut Child,
    request_frame: Vec<u8>,
    request_id: &str,
    expected_token: String,
    cancellation: &impl Fn() -> bool,
) -> Result<ComputationResponse, RunnerError> {
    let mut stdin = child.stdin.take().ok_or(RunnerError::Unavailable)?;
    let (write_sender, write_receiver) = mpsc::sync_channel(1);
    let writer = thread::spawn(move || {
        let result = std::io::Write::write_all(&mut stdin, &request_frame)
            .and_then(|()| std::io::Write::flush(&mut stdin))
            .map_err(|_| RunnerError::Unavailable);
        drop(stdin);
        let _ = write_sender.send(result);
    });
    let mut stdout = child.stdout.take().ok_or(RunnerError::Unavailable)?;
    let (sender, receiver) = mpsc::sync_channel(1);
    let reader = thread::spawn(move || {
        let result = decode_response(&mut stdout, &expected_token);
        let _ = sender.send(result);
    });
    let deadline = Instant::now() + WALL_TIME_LIMIT;
    let mut enforced_status = None;
    let response = loop {
        // Reap before querying live-task statistics. A child that exits between
        // this check and accounting is checked again below; its vanished task
        // is completion, never an accounting-policy failure for a live child.
        if child
            .try_wait()
            .map_err(|_| RunnerError::Unavailable)?
            .is_some()
        {
            break Some(response_after_exit(&receiver, deadline, cancellation));
        }
        if let Some(status) = resource_stop(child.id(), cancellation, deadline) {
            if status == RunnerStatus::PolicyUnavailable
                && (child
                    .try_wait()
                    .map_err(|_| RunnerError::Unavailable)?
                    .is_some()
                    || committed_to_exit(child.id()))
            {
                // Statistics can disappear before waitpid becomes ready. Only
                // the kernel's irreversible exit flag permits awaiting reap;
                // a live or unknown process still fails closed below.
                break Some(
                    await_exit(child, deadline, cancellation)
                        .and_then(|_| response_after_exit(&receiver, deadline, cancellation)),
                );
            }
            enforced_status = Some(status);
            break None;
        }
        match receiver.try_recv() {
            Ok(result) => break Some(result),
            Err(mpsc::TryRecvError::Disconnected) => {
                break Some(Err(RunnerError::Unavailable));
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        if let Ok(Err(error)) = write_receiver.try_recv() {
            let _ = child.kill();
            break Some(Err(error));
        }
        if child
            .try_wait()
            .map_err(|_| RunnerError::Unavailable)?
            .is_some()
        {
            break Some(response_after_exit(&receiver, deadline, cancellation));
        }
        thread::sleep(POLL_INTERVAL);
    };
    if enforced_status.is_some() {
        let _ = child.kill();
    }
    let _ = reader.join();
    let _ = writer.join();
    if let Some(status) = enforced_status {
        child.wait().map_err(|_| RunnerError::Unavailable)?;
        return Ok(ComputationResponse::terminal(request_id, status, None));
    }
    let exit = match await_exit(child, deadline, cancellation) {
        Ok(exit) => exit,
        Err(RunnerError::Cancelled) => {
            return Ok(ComputationResponse::terminal(
                request_id,
                RunnerStatus::Cancelled,
                None,
            ));
        }
        Err(RunnerError::DeadlineExceeded) => {
            return Ok(ComputationResponse::terminal(
                request_id,
                RunnerStatus::WallLimit,
                None,
            ));
        }
        Err(error) => return Err(error),
    };
    checked_response(exit, response, request_id)
}

fn checked_response(
    exit: std::process::ExitStatus,
    response: Option<Result<ComputationResponse, RunnerError>>,
    request_id: &str,
) -> Result<ComputationResponse, RunnerError> {
    if !exit.success() {
        if let Some(result) = broker_exit_result(exit, request_id) {
            return result;
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt as _;
            if exit.signal() == Some(libc::SIGXCPU) {
                return Ok(ComputationResponse::terminal(
                    request_id,
                    RunnerStatus::CpuLimit,
                    None,
                ));
            }
        }
        return Err(RunnerError::RunnerFailed);
    }
    let response = response.ok_or(RunnerError::Unavailable)??;
    response
        .validate(request_id)
        .map_err(|()| RunnerError::InvalidProtocol)?;
    Ok(response)
}

// Child exit and pipe decoding are independent events. Scheduling the bounded
// decoder can take more than one polling interval when other local workers run.
fn response_after_exit(
    receiver: &mpsc::Receiver<Result<ComputationResponse, RunnerError>>,
    deadline: Instant,
    cancellation: &impl Fn() -> bool,
) -> Result<ComputationResponse, RunnerError> {
    loop {
        if cancellation() {
            return Err(RunnerError::Cancelled);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(RunnerError::DeadlineExceeded);
        }
        match receiver.recv_timeout(remaining.min(POLL_INTERVAL)) {
            Ok(result) => return result,
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(RunnerError::RunnerFailed),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

fn broker_exit_result(
    exit: std::process::ExitStatus,
    request_id: &str,
) -> Option<Result<ComputationResponse, RunnerError>> {
    let status = match exit.code()? {
        BROKER_CPU_LIMIT_EXIT => RunnerStatus::CpuLimit,
        BROKER_MEMORY_LIMIT_EXIT => RunnerStatus::MemoryLimit,
        BROKER_POLICY_UNAVAILABLE_EXIT => RunnerStatus::PolicyUnavailable,
        BROKER_WALL_LIMIT_EXIT => RunnerStatus::WallLimit,
        BROKER_SERVICE_FAILED_EXIT => return Some(Err(RunnerError::RunnerFailed)),
        BROKER_UNAVAILABLE_EXIT => return Some(Err(RunnerError::Unavailable)),
        _ => return None,
    };
    Some(Ok(ComputationResponse::terminal(request_id, status, None)))
}

fn resource_stop(
    pid: u32,
    cancellation: &impl Fn() -> bool,
    deadline: Instant,
) -> Option<RunnerStatus> {
    if cancellation() {
        return Some(RunnerStatus::Cancelled);
    }
    if Instant::now() >= deadline {
        return Some(RunnerStatus::WallLimit);
    }
    match process_usage(pid) {
        Ok(usage) if usage.cpu_time >= CPU_TIME_LIMIT => Some(RunnerStatus::CpuLimit),
        Ok(usage) if usage.physical_bytes > PROCESS_MEMORY_LIMIT_BYTES => {
            Some(RunnerStatus::MemoryLimit)
        }
        Ok(_) => None,
        Err(()) => Some(RunnerStatus::PolicyUnavailable),
    }
}

fn await_exit(
    child: &mut Child,
    deadline: Instant,
    cancellation: &impl Fn() -> bool,
) -> Result<std::process::ExitStatus, RunnerError> {
    loop {
        if cancellation() {
            let _ = child.kill();
            return Err(RunnerError::Cancelled);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            return Err(RunnerError::DeadlineExceeded);
        }
        if let Some(exit) = child.try_wait().map_err(|_| RunnerError::Unavailable)? {
            return Ok(exit);
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn launch(executable: &Path, token: &LaunchToken) -> Result<Child, RunnerError> {
    let mut command = Command::new(executable);
    command
        .env_clear()
        .env(LAUNCH_TOKEN_ENV, token.expose_hex())
        .current_dir(Path::new("/"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command.spawn().map_err(|_| RunnerError::Unavailable)
}

fn canonical_executable(path: &Path) -> Result<PathBuf, RunnerError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| RunnerError::InvalidExecutable)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(RunnerError::InvalidExecutable);
    }
    path.canonicalize()
        .map_err(|_| RunnerError::InvalidExecutable)
}

fn terminate_and_reap(child: &mut Child) -> Result<(), RunnerError> {
    if child
        .try_wait()
        .map_err(|_| RunnerError::Unavailable)?
        .is_none()
    {
        child.kill().map_err(|_| RunnerError::Unavailable)?;
    }
    child.wait().map_err(|_| RunnerError::Unavailable)?;
    Ok(())
}

struct LaunchToken([u8; LAUNCH_TOKEN_BYTES]);

impl LaunchToken {
    fn generate() -> Result<Self, RunnerError> {
        let mut bytes = [0_u8; LAUNCH_TOKEN_BYTES];
        getrandom::fill(&mut bytes).map_err(|_| RunnerError::Unavailable)?;
        Ok(Self(bytes))
    }

    fn expose_hex(&self) -> String {
        let mut encoded = String::with_capacity(LAUNCH_TOKEN_BYTES * 2);
        for byte in self.0 {
            use fmt::Write as _;
            let _ = write!(encoded, "{byte:02x}");
        }
        encoded
    }
}

impl fmt::Debug for LaunchToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("LaunchToken([REDACTED])")
    }
}

impl Drop for LaunchToken {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

/// Sanitized host/runner boundary failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunnerError {
    /// The application-managed executable is absent, indirect or not a file.
    InvalidExecutable,
    /// The request failed the host-side contract before launch.
    InvalidRequest,
    /// A frame, version, identity or response association was malformed.
    InvalidProtocol,
    /// The request was cancelled and the process was reaped.
    Cancelled,
    /// The independent process deadline expired and the process was reaped.
    DeadlineExceeded,
    /// The runner exited without a complete authenticated response.
    RunnerFailed,
    /// Process, pipe, random or supervisor facilities were unavailable.
    Unavailable,
}

impl fmt::Display for RunnerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidExecutable => formatter.write_str("computation runner is invalid"),
            Self::InvalidRequest => formatter.write_str("computation request is invalid"),
            Self::InvalidProtocol => formatter.write_str("computation protocol is invalid"),
            Self::Cancelled => formatter.write_str("computation was cancelled"),
            Self::DeadlineExceeded => formatter.write_str("computation deadline was exceeded"),
            Self::RunnerFailed => formatter.write_str("computation runner failed"),
            Self::Unavailable => formatter.write_str("computation runner is unavailable"),
        }
    }
}

impl Error for RunnerError {}

#[cfg(test)]
mod tests;
