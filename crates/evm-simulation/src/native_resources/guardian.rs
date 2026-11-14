//! A fixed FD3 liveness pipe stops the worker when its actual owner disappears.
//! The guardian carries no request/control data and grants no output channel.
use crate::SimulationError;
use std::{
    fs::File,
    io::{self, PipeWriter, Read},
    mem,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::process::CommandExt,
    },
    process::Command,
};

const GUARDIAN: i32 = 3;

pub(crate) fn take() -> Result<File, SimulationError> {
    validate(GUARDIAN)?;
    // Do not forward this owner's liveness capability through another exec.
    // SAFETY: FD3 was verified as a live read-only pipe and belongs to this
    // single-threaded worker. Changing only its close-on-exec bit is local.
    if unsafe { libc::fcntl(GUARDIAN, libc::F_SETFD, libc::FD_CLOEXEC) } != 0 {
        return Err(SimulationError::WorkerUnavailable);
    }
    // SAFETY: validated live FD3 is exclusively transferred into this File.
    Ok(unsafe { File::from_raw_fd(GUARDIAN) })
}
fn validate(fd: i32) -> Result<(), SimulationError> {
    // SAFETY: read-only descriptor query retains no pointers or ownership.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    let mut status = mem::MaybeUninit::<libc::stat>::zeroed();
    // SAFETY: exact writable SDK output, read only after kernel success.
    if flags < 0 || unsafe { libc::fstat(fd, status.as_mut_ptr()) } != 0 {
        return Err(SimulationError::WorkerUnavailable);
    }
    // SAFETY: successful fstat initialised the complete SDK buffer.
    let status = unsafe { status.assume_init() };
    if status.st_mode & libc::S_IFMT != libc::S_IFIFO
        || flags & libc::O_ACCMODE != libc::O_RDONLY
        || flags & libc::O_NONBLOCK != 0
    {
        return Err(SimulationError::WorkerUnavailable);
    }
    Ok(())
}
pub(crate) fn start(mut pipe: File) -> Result<(), SimulationError> {
    let handle = std::thread::Builder::new()
        .name("worker-owner".into())
        .spawn(move || {
            let mut byte = [0; 1];
            loop {
                match pipe.read(&mut byte) {
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    // EOF means the actual owner vanished. Unexpected bytes
                    // and read failures are equally terminal, without logging.
                    _ => {
                        // SAFETY: this fixed worker must stop immediately; no
                        // Rust callbacks/destructors may run after owner loss.
                        unsafe { libc::_exit(1) }
                    }
                }
            }
        })
        .map_err(|_| SimulationError::WorkerUnavailable)?;
    // The process owns this dedicated guardian for its remaining lifetime.
    drop(handle);
    Ok(())
}

/// Diagnostic-only local launch uses the same mandatory guardian. Packaged
/// execution receives FD3 from the independently pinned native role broker.
pub(crate) fn configure(command: &mut Command) -> Result<PipeWriter, SimulationError> {
    let (read, write) = io::pipe().map_err(|_| SimulationError::WorkerUnavailable)?;
    if read.as_raw_fd() < GUARDIAN || write.as_raw_fd() < GUARDIAN {
        return Err(SimulationError::WorkerUnavailable);
    }
    let write_fd = write.as_raw_fd();
    // SAFETY: after fork, the closure performs only async-signal-safe descriptor
    // syscalls over these fixed inherited pipe ends. No allocation or locks.
    unsafe {
        command.pre_exec(move || {
            if (read.as_raw_fd() != GUARDIAN && libc::dup2(read.as_raw_fd(), GUARDIAN) < 0)
                || libc::fcntl(GUARDIAN, libc::F_SETFD, 0) != 0
                || libc::close(write_fd) != 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(write)
}

#[cfg(test)]
mod tests;
