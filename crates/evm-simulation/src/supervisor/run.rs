//! Shared lifecycle, with qualified packaged execution exclusively over XPC.
use super::{
    SimulationControl, SimulationStateSource, exchange,
    io::WorkerIo,
    local,
    process::{Backend, Remote, WatchedProcess},
};
use crate::{SimulationError, SimulationObservation, SimulationRequest, admission, protocol};
use std::{
    path::Path,
    sync::{
        atomic::Ordering,
        mpsc::{self, Receiver},
    },
};

/// Runs an unqualified developer diagnostic under the mandatory local profile.
///
/// # Errors
/// Rejects unsupported isolation and all protocol, state, resource or IO faults.
pub fn simulate_in_worker(
    program: &Path,
    request: &SimulationRequest,
    source: &mut impl SimulationStateSource,
    control: &SimulationControl<'_>,
) -> Result<SimulationObservation, SimulationError> {
    admission::validate(request)?;
    supervise(program, control, |input, receiver, watched| {
        exchange::exchange(request, source, control, input, receiver, watched)
    })
}
/// Executes only through the fixed, independently pinned packaged role broker.
/// No missing service or XPC error falls back to an unqualified local process.
///
/// # Errors
/// Rejects role/pin mismatches and every lifecycle, protocol or state fault.
pub fn simulate_with_trusted_worker(
    worker: &crate::TrustedSimulationWorker,
    request: &SimulationRequest,
    source: &mut impl SimulationStateSource,
    control: &SimulationControl<'_>,
) -> Result<SimulationObservation, SimulationError> {
    worker.require(crate::worker_identity::Role::Online)?;
    admission::validate(request)?;
    supervise_trusted(worker, control, |input, receiver, watched| {
        exchange::exchange(request, source, control, input, receiver, watched)
    })
}
pub(crate) fn supervise_trusted<M: serde::de::DeserializeOwned + Send, O>(
    worker: &crate::TrustedSimulationWorker,
    control: &SimulationControl<'_>,
    exchange: impl FnOnce(
        WorkerIo,
        &Receiver<Result<Option<M>, SimulationError>>,
        &WatchedProcess,
    ) -> Result<O, SimulationError>,
) -> Result<O, SimulationError> {
    control.remaining()?;
    worker.verify_file()?;
    let remote = Remote::start(worker, control)?;
    supervise_backend(Backend::Remote(remote), control, exchange)
}
pub(crate) fn supervise<M: serde::de::DeserializeOwned + Send, O>(
    program: &Path,
    control: &SimulationControl<'_>,
    exchange: impl FnOnce(
        WorkerIo,
        &Receiver<Result<Option<M>, SimulationError>>,
        &WatchedProcess,
    ) -> Result<O, SimulationError>,
) -> Result<O, SimulationError> {
    control.remaining()?;
    supervise_backend(
        Backend::Local(local::launch(program, false)?),
        control,
        exchange,
    )
}
pub(crate) fn supervise_offline<M: serde::de::DeserializeOwned + Send, O>(
    program: &Path,
    control: &SimulationControl<'_>,
    exchange: impl FnOnce(
        WorkerIo,
        &Receiver<Result<Option<M>, SimulationError>>,
        &WatchedProcess,
    ) -> Result<O, SimulationError>,
) -> Result<O, SimulationError> {
    control.remaining()?;
    supervise_backend(
        Backend::Local(local::launch(program, true)?),
        control,
        exchange,
    )
}
fn supervise_backend<M: serde::de::DeserializeOwned + Send, O>(
    backend: Backend,
    control: &SimulationControl<'_>,
    exchange: impl FnOnce(
        WorkerIo,
        &Receiver<Result<Option<M>, SimulationError>>,
        &WatchedProcess,
    ) -> Result<O, SimulationError>,
) -> Result<O, SimulationError> {
    let watched = WatchedProcess::new(backend, control.started);
    let (input, output) = watched.take_io()?;
    let input = WorkerIo::new(input, watched.io.clone())?;
    let mut output = WorkerIo::new(output, watched.io.clone())?;
    std::thread::scope(|scope| {
        let (sender, receiver) = mpsc::sync_channel(1);
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
        let execution = exchange(input, &receiver, &watched).and_then(|output| {
            watched.await_success(control)?;
            match receiver.recv_timeout(control.remaining()?) {
                Ok(Ok(None)) => Ok(output),
                _ => Err(SimulationError::Protocol),
            }
        });
        // Pipe readers/writers stop independently even when remote cleanup
        // cannot be confirmed. They must not hang this scoped host thread.
        watched.io.stopped.store(true, Ordering::Release);
        let cleanup = watched.reap();
        drop(receiver);
        let reader_joined = reader.join().is_ok();
        let monitor_joined = monitor.join().is_ok();
        cleanup?;
        if !reader_joined || !monitor_joined {
            return Err(SimulationError::WorkerUnavailable);
        }
        watched.check()?;
        execution
    })
}
