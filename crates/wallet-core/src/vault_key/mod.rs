//! Vault data-encryption-key lifecycle and platform storage.

mod authorization;
mod error;
mod key;
mod policy;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod macos_readiness;

pub use authorization::{VaultSigningAuthorization, VaultUnlockAuthorization};
pub use error::VaultKeyStoreError;
pub use key::{VAULT_ENCRYPTION_KEY_BYTES, VaultEncryptionKey, VaultKeyId};
#[cfg(target_os = "macos")]
pub use macos::MacOsVaultKeyStore;

#[cfg(test)]
mod tests;
