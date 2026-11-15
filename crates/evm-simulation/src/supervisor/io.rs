//! Nonblocking anonymous pipes keep failed remote cleanup from hanging the host.
use crate::{SimulationError, resources};
use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
use std::{
    fs::File,
    io::{self, Read, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

pub(super) struct IoState {
    started: Instant,
    pub(super) stopped: AtomicBool,
}
impl IoState {
    pub(super) fn new(started: Instant) -> Arc<Self> {
        Arc::new(Self {
            started,
            stopped: AtomicBool::new(false),
        })
    }
    fn check(&self) -> io::Result<()> {
        if self.stopped.load(Ordering::Acquire)
            || self.started.elapsed() >= resources::WALL
            || super::control::ensure_worker_runtime_available().is_err()
        {
            Err(io::Error::from(io::ErrorKind::TimedOut))
        } else {
            Ok(())
        }
    }
}
pub(crate) struct WorkerIo {
    file: File,
    state: Arc<IoState>,
}
impl WorkerIo {
    pub(super) fn new(file: File, state: Arc<IoState>) -> Result<Self, SimulationError> {
        let stat = rustix::fs::fstat(&file).map_err(|_| SimulationError::WorkerUnavailable)?;
        if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::Fifo {
            return Err(SimulationError::WorkerUnavailable);
        }
        let flags = fcntl_getfl(&file).map_err(|_| SimulationError::WorkerUnavailable)?;
        fcntl_setfl(&file, flags | OFlags::NONBLOCK)
            .map_err(|_| SimulationError::WorkerUnavailable)?;
        Ok(Self { file, state })
    }
}
impl Read for WorkerIo {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        loop {
            self.state.check()?;
            match self.file.read(bytes) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                other => return other,
            }
        }
    }
}
impl Write for WorkerIo {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        loop {
            self.state.check()?;
            match self.file.write(bytes) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                other => return other,
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        self.state.check()?;
        self.file.flush()
    }
}

#[cfg(test)]
mod tests;
