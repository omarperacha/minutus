//! Narrow macOS identity boundary. The only image inspected is the loaded main
//! host. Running-child identity belongs to the fixed-role native broker.
#![allow(unsafe_code)]
use crate::SimulationError;
#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn _dyld_get_image_header(index: u32) -> *const std::ffi::c_void;
    fn getsectiondata(
        header: *const std::ffi::c_void,
        segment: *const libc::c_char,
        section: *const libc::c_char,
        size: *mut libc::c_ulong,
    ) -> *const u8;

}
pub(crate) fn host_manifest() -> Result<Vec<u8>, SimulationError> {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: image0 is dyld's loaded main executable; fixed section names,
        // exact size output. getsectiondata returns its relocated mapped address.
        let mut size = 0;
        let pointer = unsafe {
            getsectiondata(
                _dyld_get_image_header(0),
                c"__TEXT".as_ptr(),
                c"__evm_workers".as_ptr(),
                &raw mut size,
            )
        };
        if pointer.is_null() || size == 0 || size > 1024 {
            return Err(SimulationError::WorkerUnavailable);
        }
        // SAFETY: dyld-owned immutable main-image section remains mapped for the
        // process lifetime; bounded SDK-provided section size, copied immediately.
        Ok(unsafe {
            std::slice::from_raw_parts(
                pointer,
                usize::try_from(size).map_err(|_| SimulationError::WorkerUnavailable)?,
            )
        }
        .to_vec())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(SimulationError::WorkerUnavailable)
    }
}
