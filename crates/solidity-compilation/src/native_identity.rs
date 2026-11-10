//! Narrow macOS identity boundary. The only image inspected is the loaded main
//! host, and the only process identity inspected is our own unreaped child.
#![allow(unsafe_code)]
use crate::CompilationError;
// This independent compiler boundary intentionally shares no simulator role or state broker.
#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn _dyld_get_image_header(index: u32) -> *const std::ffi::c_void;
    fn getsectiondata(
        header: *const std::ffi::c_void,
        segment: *const libc::c_char,
        section: *const libc::c_char,
        size: *mut libc::c_ulong,
    ) -> *const u8;
    fn csops(
        pid: libc::pid_t,
        operation: libc::c_uint,
        buffer: *mut std::ffi::c_void,
        size: usize,
    ) -> libc::c_int;
}
pub(crate) fn host_manifest() -> Result<Vec<u8>, CompilationError> {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: image0 is dyld's loaded main executable; fixed section names,
        // exact size output. getsectiondata returns its relocated mapped address.
        let mut size = 0;
        let pointer = unsafe {
            getsectiondata(
                _dyld_get_image_header(0),
                c"__TEXT".as_ptr(),
                c"__solc_worker".as_ptr(),
                &raw mut size,
            )
        };
        if pointer.is_null() || size == 0 || size > 1024 {
            return Err(CompilationError::WorkerUnavailable);
        }
        // SAFETY: dyld-owned immutable main-image section remains mapped for the
        // process lifetime; bounded SDK-provided section size, copied immediately.
        Ok(unsafe {
            std::slice::from_raw_parts(
                pointer,
                usize::try_from(size).map_err(|_| CompilationError::WorkerUnavailable)?,
            )
        }
        .to_vec())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(CompilationError::WorkerUnavailable)
    }
}
pub(crate) fn child_hash(pid: u32) -> Result<[u8; 20], CompilationError> {
    #[cfg(target_os = "macos")]
    {
        let pid = libc::pid_t::try_from(pid).map_err(|_| CompilationError::WorkerUnavailable)?;
        let mut flags = 0u32;
        // SAFETY: kernel writes exactly the CS_OPS_STATUS u32 into our buffer.
        // Caller retains ownership of the unreaped child, preventing PID reuse.
        if unsafe { csops(pid, 0, (&raw mut flags).cast(), size_of::<u32>()) } != 0
            || flags & 1 == 0
        {
            return Err(CompilationError::WorkerUnavailable);
        }
        let mut hash = [0u8; 20];
        // SAFETY: CS_OPS_CDHASH has a fixed20-byte result, buffer exactly sized.
        if unsafe { csops(pid, 5, hash.as_mut_ptr().cast(), hash.len()) } != 0 {
            return Err(CompilationError::WorkerUnavailable);
        }
        Ok(hash)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        Err(CompilationError::WorkerUnavailable)
    }
}

pub(crate) fn child_exited(pid: u32) -> Result<bool, CompilationError> {
    #[cfg(target_os = "macos")]
    {
        let mut status = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
        // SAFETY: exact SDK buffer; only our owned child. WNOWAIT explicitly
        // leaves its exit status and PID owned until the supervisor reaps it.
        if unsafe {
            libc::waitid(
                libc::P_PID,
                pid,
                status.as_mut_ptr(),
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        } != 0
        {
            return Err(CompilationError::WorkerUnavailable);
        }
        // SAFETY: successful waitid initialised our zeroed SDK buffer. si_pid
        // reads the SIGCHLD process field, zero when WNOHANG finds no exit.
        Ok(unsafe { status.assume_init().si_pid() } != 0)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        Err(CompilationError::WorkerUnavailable)
    }
}

pub(crate) fn physical_bytes(pid: u32) -> Result<u64, CompilationError> {
    #[cfg(target_os = "macos")]
    {
        let pid = i32::try_from(pid).map_err(|_| CompilationError::WorkerUnavailable)?;
        let mut usage = std::mem::MaybeUninit::<libc::rusage_info_v0>::zeroed();
        // SAFETY: fixed SDK buffer and the supervisor's own unreaped child only.
        if unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V0, usage.as_mut_ptr().cast()) }
            != 0
        {
            return Err(CompilationError::WorkerUnavailable);
        }
        // SAFETY: successful kernel call initialised the exact V0 structure.
        let usage = unsafe { usage.assume_init() };
        Ok(usage.ri_phys_footprint.max(usage.ri_resident_size))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        Err(CompilationError::WorkerUnavailable)
    }
}
