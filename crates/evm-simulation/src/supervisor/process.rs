//! Own the actual local diagnostic or remotely supervised fixed worker lifecycle.
use super::{SimulationControl, control, io::IoState, local::ReapedChild};
use crate::{SimulationError, resources};
use std::{
    fs::File,
    os::fd::OwnedFd,
    sync::{Arc, Mutex, atomic::Ordering},
    time::Duration,
};
use worker_transport::{ReadyChannel, Role};

pub(super) enum Backend {
    Local(ReapedChild),
    Remote(Remote),
}
pub(super) struct Remote {
    channel: ReadyChannel,
    offline: bool,
    reaped: bool,
}
impl Remote {
    pub(super) fn start(
        identity: &crate::TrustedSimulationWorker,
        control: &SimulationControl<'_>,
    ) -> Result<Self, SimulationError> {
        let offline = identity.transport_role() == Role::BaseOffline;
        control.remaining()?;
        let worker = identity.transport()?;
        let remaining = control.remaining()?;
        let channel = worker
            .start_with_deadline(remaining, control.cancelled)
            .map_err(|error| map_remote_error(error, offline))?;
        Ok(Self {
            channel,
            offline,
            reaped: false,
        })
    }
    fn map(&self, error: worker_transport::Error) -> SimulationError {
        map_remote_error(error, self.offline)
    }
    fn finish(&mut self, control: &SimulationControl<'_>) -> Result<(), SimulationError> {
        let receipt = self
            .channel
            .finish_with_deadline(control.remaining()?, control.cancelled)
            .map_err(|error| self.map(error))?;
        self.reaped = true;
        if receipt.success() {
            Ok(())
        } else {
            Err(SimulationError::WorkerUnavailable)
        }
    }
    fn reap(&mut self) -> Result<(), SimulationError> {
        if self.reaped {
            return self.channel.check().map_err(|error| self.map(error));
        }
        if let Err(error) = self.channel.cancel_and_reap() {
            // Every cleanup error means no actual-reap receipt, regardless of
            // its diagnostic category. Signed authority cannot resume online.
            if self.offline {
                control::quarantine();
            }
            return Err(self.map(error));
        }
        self.reaped = true;
        Ok(())
    }
}
impl Drop for Remote {
    fn drop(&mut self) {
        let _ = self.reap();
    }
}
fn map_remote_error(error: worker_transport::Error, offline: bool) -> SimulationError {
    match error {
        worker_transport::Error::UnconfirmedTermination => {
            if offline {
                control::quarantine();
            }
            SimulationError::WorkerUnavailable
        }
        worker_transport::Error::Cancelled => SimulationError::Cancelled,
        worker_transport::Error::Limit => SimulationError::Limit,
        worker_transport::Error::Protocol => SimulationError::Protocol,
        worker_transport::Error::Unavailable => SimulationError::WorkerUnavailable,
    }
}
impl Backend {
    pub(super) fn take_io(&mut self) -> Result<(File, File), SimulationError> {
        match self {
            Self::Local(child) => Ok((
                File::from(OwnedFd::from(
                    child
                        .stdin
                        .take()
                        .ok_or(SimulationError::WorkerUnavailable)?,
                )),
                File::from(OwnedFd::from(
                    child
                        .stdout
                        .take()
                        .ok_or(SimulationError::WorkerUnavailable)?,
                )),
            )),
            Self::Remote(remote) => {
                let input = remote
                    .channel
                    .take_stdin()
                    .map_err(|error| remote.map(error))?;
                let output = remote
                    .channel
                    .take_stdout()
                    .map_err(|error| remote.map(error))?;
                Ok((input, output))
            }
        }
    }
    fn poll(&mut self) -> Result<bool, SimulationError> {
        match self {
            Self::Local(child) => {
                if child
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
            }
            Self::Remote(remote) => {
                remote.channel.check().map_err(|error| remote.map(error))?;
                Ok(true)
            }
        }
    }
    fn reap(&mut self) -> Result<(), SimulationError> {
        match self {
            Self::Local(child) => child.reap(),
            Self::Remote(remote) => remote.reap(),
        }
    }
}

pub(crate) struct WatchedProcess {
    backend: Mutex<Backend>,
    pub(super) io: Arc<IoState>,
    fault: Mutex<Option<SimulationError>>,
}
impl WatchedProcess {
    pub(super) fn new(backend: Backend, started: std::time::Instant) -> Self {
        Self {
            backend: Mutex::new(backend),
            io: IoState::new(started),
            fault: Mutex::new(None),
        }
    }
    pub(super) fn take_io(&self) -> Result<(File, File), SimulationError> {
        self.backend
            .lock()
            .map_err(|_| SimulationError::WorkerUnavailable)?
            .take_io()
    }
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
    pub(super) fn monitor(&self, control: &SimulationControl<'_>) {
        while !self.io.stopped.load(Ordering::Acquire) {
            let check = control.remaining().and_then(|_| {
                self.backend
                    .lock()
                    .map_err(|_| SimulationError::WorkerUnavailable)?
                    .poll()
            });
            match check {
                Ok(true) => {}
                Ok(false) => break,
                Err(error) => {
                    self.fail(error);
                    let _ = self.reap();
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    pub(super) fn fail(&self, error: SimulationError) {
        if let Ok(mut fault) = self.fault.lock() {
            *fault = Some(error);
        }
        self.io.stopped.store(true, Ordering::Release);
    }
    pub(super) fn reap(&self) -> Result<(), SimulationError> {
        self.backend
            .lock()
            .map_err(|_| SimulationError::WorkerUnavailable)?
            .reap()
    }
    pub(super) fn await_success(
        &self,
        control: &SimulationControl<'_>,
    ) -> Result<(), SimulationError> {
        loop {
            control.remaining()?;
            self.check()?;
            {
                let mut backend = self
                    .backend
                    .lock()
                    .map_err(|_| SimulationError::WorkerUnavailable)?;
                match &mut *backend {
                    Backend::Remote(remote) => return remote.finish(control),
                    Backend::Local(child) => {
                        if let Some(status) = child
                            .try_wait()
                            .map_err(|_| SimulationError::WorkerUnavailable)?
                        {
                            return if status.success() {
                                Ok(())
                            } else {
                                Err(SimulationError::WorkerUnavailable)
                            };
                        }
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

#[cfg(test)]
mod tests;
