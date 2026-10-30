//! Narrow audited OS boundary, confined to resource policy and descriptor closure.
//! Exception: native read-only accounting for our own unreaped child and closing
//! inherited descriptors in the single-threaded worker require libc FFI. No
//! arbitrary PID, descriptor or memory-reading API is public or model-visible.
#![allow(unsafe_code)]
use crate::SimulationError;

pub(crate) fn close_inherited_descriptors() -> Result<(), SimulationError> {
    #[cfg(target_os = "macos")]
    {
        // Darwin has no closefrom symbol. Enumerate only our own descriptors
        // before starting any threads, then close every non-stdio entry.
        // SAFETY: size-only self query retains no pointer and opens no descriptor.
        let bytes = unsafe {
            libc::proc_pidinfo(
                libc::getpid(),
                libc::PROC_PIDLISTFDS,
                0,
                std::ptr::null_mut(),
                0,
            )
        };
        let size = std::mem::size_of::<libc::proc_fdinfo>();
        let bytes = usize::try_from(bytes).map_err(|_| SimulationError::WorkerUnavailable)?;
        if bytes == 0 || bytes > 16_384 * size || !bytes.is_multiple_of(size) {
            return Err(SimulationError::WorkerUnavailable);
        }
        // SAFETY: proc_fdinfo has scalar integer fields and valid all-zero state.
        let mut entries: Vec<libc::proc_fdinfo> = (0..bytes / size)
            .map(|_| unsafe { std::mem::zeroed() })
            .collect();
        // SAFETY: exact writable SDK buffer, self PID, single-threaded startup.
        let written = unsafe {
            libc::proc_pidinfo(
                libc::getpid(),
                libc::PROC_PIDLISTFDS,
                0,
                entries.as_mut_ptr().cast(),
                i32::try_from(bytes).map_err(|_| SimulationError::WorkerUnavailable)?,
            )
        };
        if usize::try_from(written).is_err()
            || written == 0
            || usize::try_from(written)
                .is_ok_and(|written| written > bytes || !written.is_multiple_of(size))
        {
            return Err(SimulationError::WorkerUnavailable);
        }
        entries.truncate(
            usize::try_from(written).map_err(|_| SimulationError::WorkerUnavailable)? / size,
        );
        for entry in entries {
            if entry.proc_fd >= 3 {
                // SAFETY: this worker owns the inherited descriptor and never
                // borrowed it into Rust IO. Stdio handles are deliberately kept.
                if unsafe { libc::close(entry.proc_fd) } != 0 {
                    return Err(SimulationError::WorkerUnavailable);
                }
            }
        }
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        // SAFETY: closes only this worker's inherited descriptors >=3. No
        // borrowed descriptors or other threads exist at this startup boundary.
        let result = unsafe { libc::syscall(libc::SYS_close_range, 3_u32, u32::MAX, 0_u32) };
        if result == 0 {
            Ok(())
        } else {
            Err(SimulationError::WorkerUnavailable)
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Err(SimulationError::WorkerUnavailable)
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn physical_bytes(pid: u32) -> Result<u64, SimulationError> {
    let pid = i32::try_from(pid).map_err(|_| SimulationError::WorkerUnavailable)?;
    let mut usage = std::mem::MaybeUninit::<libc::rusage_info_v0>::zeroed();
    // SAFETY: exact SDK V0 buffer; caller owns this unreaped child and supplies
    // no model-controlled PID. Read only after a successful complete write.
    if unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V0, usage.as_mut_ptr().cast()) } != 0 {
        return Err(SimulationError::WorkerUnavailable);
    }
    // SAFETY: successful kernel call initialised the exact V0 structure.
    let usage = unsafe { usage.assume_init() };
    Ok(usage.ri_phys_footprint.max(usage.ri_resident_size))
}
#[cfg(not(target_os = "macos"))]
pub(crate) fn physical_bytes(_pid: u32) -> Result<u64, SimulationError> {
    Err(SimulationError::WorkerUnavailable)
}
