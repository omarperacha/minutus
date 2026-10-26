//! Non-secret native lifecycle notification registration.
//! No wallet, credential, IPC or signing authority enters this leaf crate.

#[cfg(target_os = "macos")]
mod registration;
#[cfg(target_os = "macos")]
pub use registration::{LifecycleEvent, LifecycleRegistrationError, install_lock_notifications};
