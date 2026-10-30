//! Host-only one-shot supervisor. State acquisition stays outside the engine.
use crate::{
    admission,
    contract::{
        MAX_STATE_BYTES, MAX_STATE_QUERIES, SimulationError, SimulationObservation,
        SimulationRequest, StateQuery, StateValue,
    },
    protocol::{self, HostMessage, WorkerMessage},
    resources,
};
use std::{
    io,
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// Deadline/cancellation context for the trusted state-acquisition adapter.
pub struct SimulationControl<'a> {
    cancelled: &'a AtomicBool,
    started: Instant,
}
impl<'a> SimulationControl<'a> {
    /// Starts the fixed total run budget before state acquisition.
    #[must_use]
    pub fn start(cancelled: &'a AtomicBool) -> Self {
        Self {
            cancelled,
            started: Instant::now(),
        }
    }
    /// Remaining total run allowance. Adapters must use this as their IO timeout.
    ///
    /// # Errors
    /// Rejects cancellation or expired execution before any new external query.
    pub fn remaining(&self) -> Result<Duration, SimulationError> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err(SimulationError::Cancelled);
        }
        resources::WALL
            .checked_sub(self.started.elapsed())
            .filter(|left| !left.is_zero())
            .ok_or(SimulationError::Limit)
    }
}
/// Trusted state broker: implementations must bind every answer to the request's
/// exact block, respect `remaining` during IO and check consistency/canonicality.
/// The engine cannot supply URLs, alternate blocks, filesystem paths or sends.
pub trait SimulationStateSource {
    /// Obtains one bounded, exact-state answer under the current run deadline.
    ///
    /// # Errors
    /// Missing/inconsistent state and deadline/cancellation fail the entire run.
    fn query(
        &mut self,
        query: &StateQuery,
        control: &SimulationControl<'_>,
    ) -> Result<StateValue, SimulationError>;
}

const SANDBOX: &str = r#"(version 1)
(deny default)
(allow process-exec (literal (param "WORKER")))
(allow file-read-metadata)
; Darwin loader opens the root directory; this literal grants no child files.
(allow file-read-data (literal "/"))
(allow file-read-data (literal (param "WORKER")) (subpath "/System/Library") (subpath "/usr/lib"))
(allow sysctl-read)
(allow process-info* (target self))
(allow signal (target self))
"#;

fn launch(program: &Path) -> Result<Child, SimulationError> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = program;
        Err(SimulationError::WorkerUnavailable)
    }
    #[cfg(target_os = "macos")]
    {
        let program = program
            .canonicalize()
            .map_err(|_| SimulationError::WorkerUnavailable)?;
        // OS sandbox is mandatory, not an opportunistic best-effort fallback.
        Command::new("/usr/bin/sandbox-exec")
            .args(["-p", SANDBOX, "-D"])
            .arg(format!(
                "WORKER={}",
                program.to_str().ok_or(SimulationError::WorkerUnavailable)?
            ))
            .arg(program)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| SimulationError::WorkerUnavailable)
    }
}

/// Runs a trusted host-selected helper binary under fixed sandbox/process limits.
/// This developer boundary is not yet wired to packaged app signing/review.
///
/// # Errors
/// Kills/reaps on cancellation, timeout, resource/protocol/state faults. Returns
/// only after successful worker exit; no detached engine work survives return.
pub fn simulate_in_worker(
    program: &Path,
    request: &SimulationRequest,
    source: &mut impl SimulationStateSource,
    control: &SimulationControl<'_>,
) -> Result<SimulationObservation, SimulationError> {
    admission::validate(request)?;
    supervise(program, control, |input, receiver, watched| {
        exchange(request, source, control, input, receiver, watched)
    })
}

/// Runs the online helper pinned into the loaded host build, rechecking both
/// file digest before spawn and kernel running-image identity before any input.
///
/// # Errors
/// Rejects wrong helper role, identity changes and all ordinary worker failures.
pub fn simulate_with_trusted_worker(
    worker: &crate::TrustedSimulationWorker,
    request: &SimulationRequest,
    source: &mut impl SimulationStateSource,
    control: &SimulationControl<'_>,
) -> Result<SimulationObservation, SimulationError> {
    worker.require(crate::worker_identity::Role::Online)?;
    admission::validate(request)?;
    supervise_trusted(worker, control, |input, receiver, watched| {
        exchange(request, source, control, input, receiver, watched)
    })
}
pub(crate) fn supervise_trusted<M: serde::de::DeserializeOwned + Send, O>(
    worker: &crate::TrustedSimulationWorker,
    control: &SimulationControl<'_>,
    exchange: impl FnOnce(
        std::process::ChildStdin,
        &std::sync::mpsc::Receiver<Result<Option<M>, SimulationError>>,
        &WatchedChild,
    ) -> Result<O, SimulationError>,
) -> Result<O, SimulationError> {
    worker.verify_file()?;
    supervise_checked(worker.path(), Some(worker), control, exchange)
}
// Both protocols use exactly the same mandatory process policy. The offline
// protocol cannot receive a state source or invoke the online exchange.
pub(crate) fn supervise<M: serde::de::DeserializeOwned + Send, O>(
    program: &Path,
    control: &SimulationControl<'_>,
    exchange: impl FnOnce(
        std::process::ChildStdin,
        &std::sync::mpsc::Receiver<Result<Option<M>, SimulationError>>,
        &WatchedChild,
    ) -> Result<O, SimulationError>,
) -> Result<O, SimulationError> {
    supervise_checked(program, None, control, exchange)
}
fn supervise_checked<M: serde::de::DeserializeOwned + Send, O>(
    program: &Path,
    identity: Option<&crate::TrustedSimulationWorker>,
    control: &SimulationControl<'_>,
    exchange: impl FnOnce(
        std::process::ChildStdin,
        &std::sync::mpsc::Receiver<Result<Option<M>, SimulationError>>,
        &WatchedChild,
    ) -> Result<O, SimulationError>,
) -> Result<O, SimulationError> {
    control.remaining()?;
    let mut process = ReapedChild(launch(program)?);
    let input = process
        .stdin
        .take()
        .ok_or(SimulationError::WorkerUnavailable)?;
    let mut output = process
        .stdout
        .take()
        .ok_or(SimulationError::WorkerUnavailable)?;
    let watched = WatchedChild {
        child: Mutex::new(process),
        finished: AtomicBool::new(false),
        identity_checked: AtomicBool::new(identity.is_none()),
        fault: Mutex::new(None),
    };
    thread::scope(|scope| {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let reader = scope.spawn(move || {
            loop {
                let message = protocol::read_next_frame::<M>(&mut output);
                let terminal = !matches!(message, Ok(Some(_)));
                if sender.send(message).is_err() || terminal {
                    break;
                }
            }
        });
        let monitor = scope.spawn(|| watched.monitor(control));
        let attested = identity.map_or(Ok(()), |worker| {
            let pid = watched
                .child
                .lock()
                .map_err(|_| SimulationError::WorkerUnavailable)?
                .id();
            worker.await_running(pid, control)
        });
        watched.identity_checked.store(true, Ordering::Release);
        let execution = attested
            .and_then(|()| exchange(input, &receiver, &watched))
            .and_then(|output| {
                watched.await_success(control)?;
                // A successful protocol ends with exactly one terminal response and
                // clean EOF. Extra frames and malformed trailing bytes fail closed.
                match receiver.recv_timeout(control.remaining()?) {
                    Ok(Ok(None)) => Ok(output),
                    _ => Err(SimulationError::Protocol),
                }
            });
        watched.finished.store(true, Ordering::Release);
        watched.reap();
        drop(receiver);
        let _ = reader.join();
        let _ = monitor.join();
        watched.check()?;
        execution
    })
}

// Monitoring never waits for the trusted broker; it can kill the engine during
// state IO. The broker independently honours the shared deadline/cancellation.
pub(crate) struct WatchedChild {
    child: Mutex<ReapedChild>,
    finished: AtomicBool,
    // try_wait reaps. Keep the PID owned until all kernel identity reads finish.
    identity_checked: AtomicBool,
    fault: Mutex<Option<SimulationError>>,
}
impl WatchedChild {
    pub(crate) fn check(&self) -> Result<(), SimulationError> {
        match *self
            .fault
            .lock()
            .map_err(|_| SimulationError::WorkerUnavailable)?
        {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
    fn monitor(&self, control: &SimulationControl<'_>) {
        while !self.finished.load(Ordering::Acquire) {
            let check = control.remaining().and_then(|_| {
                let mut child = self
                    .child
                    .lock()
                    .map_err(|_| SimulationError::WorkerUnavailable)?;
                if self.identity_checked.load(Ordering::Acquire)
                    && child
                        .try_wait()
                        .map_err(|_| SimulationError::WorkerUnavailable)?
                        .is_some()
                {
                    return Ok(false);
                }
                if crate::native_resources::physical_bytes(child.id())? > resources::MEMORY {
                    return Err(SimulationError::Limit);
                }
                Ok(true)
            });
            match check {
                Ok(true) => {}
                Ok(false) => break,
                Err(error) => {
                    if let Ok(mut observed) = self.fault.lock() {
                        *observed = Some(error);
                    }
                    if let Ok(mut child) = self.child.lock() {
                        let _ = child.kill();
                    }
                    break;
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn reap(&self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
    fn await_success(&self, control: &SimulationControl<'_>) -> Result<(), SimulationError> {
        loop {
            control.remaining()?;
            self.check()?;
            if let Some(status) = self
                .child
                .lock()
                .map_err(|_| SimulationError::WorkerUnavailable)?
                .try_wait()
                .map_err(|_| SimulationError::WorkerUnavailable)?
            {
                return if status.success() {
                    Ok(())
                } else {
                    Err(SimulationError::WorkerUnavailable)
                };
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
}

fn exchange(
    request: &SimulationRequest,
    source: &mut impl SimulationStateSource,
    control: &SimulationControl<'_>,
    mut input: impl io::Write,
    receiver: &std::sync::mpsc::Receiver<Result<Option<WorkerMessage>, SimulationError>>,
    watched: &WatchedChild,
) -> Result<SimulationObservation, SimulationError> {
    protocol::write_frame(
        &HostMessage::Start {
            version: 1,
            request: Box::new(request.clone()),
        },
        &mut input,
    )?;
    let mut count = 0;
    let mut total_bytes = 0;
    let observation = loop {
        control.remaining()?;
        watched.check()?;
        let message = match receiver.recv_timeout(Duration::from_millis(10)) {
            Ok(value) => value?.ok_or(SimulationError::Protocol)?,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => return Err(SimulationError::Protocol),
        };
        match message {
            WorkerMessage::State { sequence, query } => {
                count += 1;
                if count > MAX_STATE_QUERIES || sequence != count {
                    return Err(SimulationError::Limit);
                }
                admission::validate_query(&query, request.block.number)?;
                let value = source.query(&query, control)?;
                control.remaining()?;
                total_bytes +=
                    protocol::write_frame(&HostMessage::State { sequence, value }, &mut input)?;
                if total_bytes > MAX_STATE_BYTES {
                    return Err(SimulationError::Limit);
                }
            }
            WorkerMessage::Finished { result } => break result?,
        }
    };
    if observation.block_hash != request.block.hash
        || observation.request_commitment != protocol::commitment(request)?
        || observation.effects_qualified
    {
        return Err(SimulationError::Protocol);
    }
    drop(input);
    Ok(observation)
}

// Keep io errors local: provider/pipe diagnostics never become model prose.
impl From<io::Error> for SimulationError {
    fn from(_: io::Error) -> Self {
        Self::Protocol
    }
}

// Own the exact unreaped process even on early pipe setup errors/unwinding.
struct ReapedChild(Child);
impl std::ops::Deref for ReapedChild {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.0
    }
}
impl std::ops::DerefMut for ReapedChild {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}
impl Drop for ReapedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
