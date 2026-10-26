//! Versioned, authenticated encrypted-vault envelope.

mod record;
mod validation;

pub use record::{AccountKeyOrigin, AccountMetadata, VaultDocument};
pub use validation::VaultFormatError;

/// Vault format emitted by this build.
pub const CURRENT_VAULT_VERSION: u32 = 1;
/// Authenticated-encryption algorithm required by format version 1.
pub const ENCRYPTION_ALGORITHM: &str = "xchacha20-poly1305";
/// Key-derivation algorithm required by format version 1.
pub const KDF_ALGORITHM: &str = "hkdf-sha256";
/// Domain-separation context required by format version 1.
pub const KDF_INFO: &str = "zeta-wallet-vault/v1";
/// XChaCha20-Poly1305 nonce size.
pub const NONCE_BYTES: usize = 24;
/// HKDF-SHA-256 salt size.
pub const KDF_SALT_BYTES: usize = 32;
/// Poly1305 authentication-tag size.
pub const AUTHENTICATION_TAG_BYTES: usize = 16;
/// Maximum encrypted secret payload accepted from a vault file.
pub const MAX_ENCRYPTED_PAYLOAD_BYTES: usize = 1024 * 1024;
/// Maximum number of public account records in one vault.
pub const MAX_ACCOUNTS: usize = 64;

const MAX_DOCUMENT_BYTES: usize = 2 * 1024 * 1024;
const MAX_ACCOUNT_ID_BYTES: usize = 64;
const MAX_DERIVATION_PATH_BYTES: usize = 128;

#[cfg(test)]
mod tests;
