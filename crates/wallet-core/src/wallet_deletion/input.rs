use zeroize::Zeroizing;

use super::VaultDeletionError;

const MAX_TYPED_CONFIRMATION_BYTES: usize = 71;

/// Owned, zeroizing typed confirmation from the trusted deletion surface.
///
/// This type intentionally implements neither cloning, debugging, formatting
/// nor serialization.
pub struct VaultTypedConfirmation(Zeroizing<String>);

impl VaultTypedConfirmation {
    /// Takes ownership of bounded printable ASCII confirmation input.
    ///
    /// # Errors
    ///
    /// Returns [`VaultDeletionError::InvalidTypedConfirmation`] when input is
    /// empty, oversized or contains control or non-ASCII characters.
    pub fn new(value: String) -> Result<Self, VaultDeletionError> {
        let value = Zeroizing::new(value);
        if value.is_empty()
            || value.len() > MAX_TYPED_CONFIRMATION_BYTES
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_graphic() || byte == b' ')
        {
            return Err(VaultDeletionError::InvalidTypedConfirmation);
        }
        Ok(Self(value))
    }

    pub(crate) fn into_value(self) -> Zeroizing<String> {
        self.0
    }
}
