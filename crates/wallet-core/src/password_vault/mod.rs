//! Versioned password protection of the existing random wallet DEK.

mod authentication;
mod credential;
mod envelope;
mod error;
pub(crate) mod kdf;
mod signing;
#[cfg(unix)]
pub(crate) mod storage;
pub(crate) mod work;

pub use authentication::PasswordVaultUnlock;
pub use credential::{MAX_WALLET_PASSWORD_BYTES, MIN_WALLET_PASSWORD_CHARACTERS, WalletPassword};
pub use envelope::{MAX_PASSWORD_VAULT_BYTES, PASSWORD_VAULT_VERSION, PasswordVaultDocument};
pub use error::PasswordVaultError;
pub use signing::PasswordSigningAttempt;
#[cfg(unix)]
pub use storage::PasswordVaultStore;
pub use work::{PasswordVaultJob, PasswordVaultWork};

#[cfg(test)]
mod tests;
