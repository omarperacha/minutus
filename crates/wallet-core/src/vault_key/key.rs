use zeroize::Zeroizing;

use super::VaultKeyStoreError;

/// Size of the random XChaCha20-Poly1305 data-encryption key.
pub const VAULT_ENCRYPTION_KEY_BYTES: usize = 32;

const MAX_VAULT_KEY_ID_BYTES: usize = 64;

/// Validated opaque identifier used as the Keychain account name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VaultKeyId(String);

impl VaultKeyId {
    /// Creates a bounded identifier containing only stable ASCII characters.
    ///
    /// # Errors
    ///
    /// Returns [`VaultKeyStoreError::InvalidKeyId`] when the identifier is empty,
    /// oversized or contains a character outside `[A-Za-z0-9_-]`.
    pub fn new(value: impl Into<String>) -> Result<Self, VaultKeyStoreError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_VAULT_KEY_ID_BYTES
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(VaultKeyStoreError::InvalidKeyId);
        }
        Ok(Self(value))
    }

    /// Returns the non-secret Keychain account identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Zeroizing 256-bit vault data-encryption key.
///
/// This type intentionally implements neither `Clone`, `Debug` nor serialization.
/// It is separate from every vault document and public metadata type.
pub struct VaultEncryptionKey(Zeroizing<[u8; VAULT_ENCRYPTION_KEY_BYTES]>);

impl VaultEncryptionKey {
    pub(crate) fn from_password_bytes(bytes: Zeroizing<[u8; VAULT_ENCRYPTION_KEY_BYTES]>) -> Self {
        Self(bytes)
    }
    /// Generates a key using the operating system cryptographic random source.
    ///
    /// # Errors
    ///
    /// Returns [`VaultKeyStoreError::RandomnessUnavailable`] if the operating
    /// system cannot fill the complete key buffer.
    pub fn generate() -> Result<Self, VaultKeyStoreError> {
        let mut bytes = Zeroizing::new([0; VAULT_ENCRYPTION_KEY_BYTES]);
        getrandom::fill(bytes.as_mut()).map_err(|_| VaultKeyStoreError::RandomnessUnavailable)?;
        Ok(Self(bytes))
    }

    /// Borrows the key only for an owning cryptographic or Keychain operation.
    #[must_use]
    pub fn expose_secret(&self) -> &[u8; VAULT_ENCRYPTION_KEY_BYTES] {
        &self.0
    }

    pub(super) fn from_bytes(bytes: [u8; VAULT_ENCRYPTION_KEY_BYTES]) -> Self {
        Self(Zeroizing::new(bytes))
    }
}
