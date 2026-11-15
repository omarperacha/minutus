//! Narrow opaque XPC ownership/descriptor boundary. No public raw handle exists.
#![allow(unsafe_code)]
use crate::{Error, identity::Identity};
use std::fs::File;
#[cfg(target_os = "macos")]
use std::{
    ffi::{CString, c_void},
    ptr::NonNull,
};
#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn wt_open(
        service: *const libc::c_char,
        role: *const libc::c_char,
        hash: *const libc::c_char,
        nonce: *const libc::c_char,
        out: *mut *mut c_void,
    ) -> i32;
    fn wt_ready(client: *mut c_void, wait_ms: u32, input: *mut i32, output: *mut i32) -> i32;
    fn wt_check(client: *mut c_void) -> i32;
    fn wt_finish(
        client: *mut c_void,
        cancel: bool,
        wait_ms: u32,
        exit: *mut i32,
        resource: *mut i32,
    ) -> i32;
    fn wt_close(client: *mut c_void);
    fn wt_host_manifest(output: *mut u8, capacity: usize, length: *mut usize) -> i32;
}
#[cfg(target_os = "macos")]
pub(crate) struct Client(NonNull<c_void>);
#[cfg(not(target_os = "macos"))]
pub(crate) struct Client;
// SAFETY: native state has atomic lifetime ownership and a mutex around all
// callbacks/control state. Rust exclusively owns this handle and never exports it.
#[cfg(target_os = "macos")]
unsafe impl Send for Client {}
impl Client {
    pub(crate) fn open(identity: &Identity, role: &str, nonce: &str) -> Result<Self, Error> {
        #[cfg(target_os = "macos")]
        {
            let service =
                CString::new(identity.service.as_str()).map_err(|_| Error::Unavailable)?;
            let role = CString::new(role).map_err(|_| Error::Unavailable)?;
            let hash =
                CString::new(identity.broker_cdhash.as_str()).map_err(|_| Error::Unavailable)?;
            let nonce = CString::new(nonce).map_err(|_| Error::Unavailable)?;
            let mut pointer = std::ptr::null_mut();
            // SAFETY: all strings/buffer live through synchronous copy; native
            // installs the kernel peer requirement before resuming connection.
            let result = unsafe {
                wt_open(
                    service.as_ptr(),
                    role.as_ptr(),
                    hash.as_ptr(),
                    nonce.as_ptr(),
                    &raw mut pointer,
                )
            };
            if result != 0 {
                return Err(Error::Unavailable);
            }
            Ok(Self(NonNull::new(pointer).ok_or(Error::Unavailable)?))
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (&identity.service, &identity.broker_cdhash, role, nonce);
            Err(Error::Unavailable)
        }
    }
    pub(crate) fn ready(&self) -> Result<Option<(File, File)>, Error> {
        #[cfg(target_os = "macos")]
        {
            use std::os::fd::FromRawFd;
            let (mut input, mut output) = (-1, -1);
            // SAFETY: exact writable scalars; successful descriptors are unique
            // native-owned anonymous pipes transferred exactly once.
            let result = unsafe { wt_ready(self.0.as_ptr(), 20, &raw mut input, &raw mut output) };
            if result == 3 {
                return Ok(None);
            }
            if result != 0 {
                return Err(Error::Unavailable);
            }
            // SAFETY: successful native ready validated type/mode/CLOEXEC and
            // relinquished both descriptors. File takes each exactly once.
            Ok(Some(unsafe {
                (File::from_raw_fd(input), File::from_raw_fd(output))
            }))
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(Error::Unavailable)
        }
    }
    pub(crate) fn check(&self) -> Result<(), Error> {
        #[cfg(target_os = "macos")]
        {
            // SAFETY: live exclusively owned opaque handle; no data input.
            if unsafe { wt_check(self.0.as_ptr()) } == 0 {
                Ok(())
            } else {
                Err(Error::UnconfirmedTermination)
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(Error::Unavailable)
        }
    }
    pub(crate) fn finish(&self, cancel: bool) -> Result<Option<(i32, i32)>, Error> {
        #[cfg(target_os = "macos")]
        {
            let (mut exit, mut resource) = (0, 0);
            // SAFETY: exact scalars. Only authenticated nonce-bound reaped:true
            // can return success; failure never fabricates a cleanup receipt.
            match unsafe {
                wt_finish(
                    self.0.as_ptr(),
                    cancel,
                    20,
                    &raw mut exit,
                    &raw mut resource,
                )
            } {
                0 => Ok(Some((exit, resource))),
                3 => Ok(None),
                _ => Err(Error::UnconfirmedTermination),
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = cancel;
            Err(Error::Unavailable)
        }
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        // SAFETY: releases only our native reference; pending callbacks retain
        // their own references until completion/finalizer and cannot use freed data.
        unsafe {
            wt_close(self.0.as_ptr());
        }
    }
}
pub(crate) fn manifest() -> Result<Vec<u8>, Error> {
    #[cfg(target_os = "macos")]
    {
        let mut bytes = vec![0; 4096];
        let mut length = 0;
        // SAFETY: bounded owned buffer; native copies only the loaded main-image
        // immutable section. No runtime file/sidecar can substitute pins.
        if unsafe { wt_host_manifest(bytes.as_mut_ptr(), bytes.len(), &raw mut length) } != 0
            || length == 0
            || length > bytes.len()
        {
            return Err(Error::Unavailable);
        }
        bytes.truncate(length);
        Ok(bytes)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(Error::Unavailable)
    }
}
