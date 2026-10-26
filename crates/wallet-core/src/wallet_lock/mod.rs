//! In-memory vault lock state, inactivity policy and signing reauthorization.

mod error;
mod session;

pub use error::VaultSessionError;
pub use session::{
    MAX_INACTIVITY_TIMEOUT, MIN_INACTIVITY_TIMEOUT, VaultInactivityTimeout, VaultLockReason,
    VaultSession, VaultSessionStatus, VaultSigningPermit,
};

#[cfg(test)]
mod tests;
