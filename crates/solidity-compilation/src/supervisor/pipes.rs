//! Host-deadline IO cannot remain blocked after missing remote cleanup evidence.
use crate::CompilationError;
use std::{
    fs::File,
    io::{self, Read, Write},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};
pub(super) struct TimedPipe<'a> {
    file: File,
    started: Instant,
    cancelled: &'a AtomicBool,
    stop: &'a AtomicBool,
}
impl<'a> TimedPipe<'a> {
    pub(super) fn new(
        file: File,
        started: Instant,
        cancelled: &'a AtomicBool,
        stop: &'a AtomicBool,
    ) -> Result<Self, CompilationError> {
        let flags =
            rustix::fs::fcntl_getfl(&file).map_err(|_| CompilationError::WorkerUnavailable)?;
        rustix::fs::fcntl_setfl(&file, flags | rustix::fs::OFlags::NONBLOCK)
            .map_err(|_| CompilationError::WorkerUnavailable)?;
        Ok(Self {
            file,
            started,
            cancelled,
            stop,
        })
    }
    fn ready(&self) -> io::Result<()> {
        if self.stop.load(Ordering::Acquire)
            || self.cancelled.load(Ordering::Acquire)
            || self.started.elapsed() >= super::WALL
        {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Compiler IO stopped",
            ));
        }
        Ok(())
    }
}
impl Read for TimedPipe<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        loop {
            self.ready()?;
            match self.file.read(buffer) {
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) =>
                {
                    thread::sleep(Duration::from_millis(5));
                }
                result => return result,
            }
        }
    }
}
impl Write for TimedPipe<'_> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        loop {
            self.ready()?;
            match self.file.write(buffer) {
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) =>
                {
                    thread::sleep(Duration::from_millis(5));
                }
                result => return result,
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        self.ready()
    }
}
