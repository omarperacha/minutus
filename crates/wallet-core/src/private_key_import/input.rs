use zeroize::Zeroizing;

use super::EvmPrivateKeyImportError;

const MAX_PRIVATE_KEY_INPUT_BYTES: usize = 66;

/// Owned, zeroizing EVM private-key text received by the native import surface.
///
/// This type intentionally implements neither cloning, debugging, formatting
/// nor serialization and has no IPC representation.
pub struct EvmPrivateKeyInput(Zeroizing<String>);

impl EvmPrivateKeyInput {
    /// Takes ownership of bounded ASCII input.
    ///
    /// # Errors
    ///
    /// Returns [`EvmPrivateKeyImportError::InvalidInput`] when input is empty,
    /// longer than a prefixed 32-byte hexadecimal key or contains non-ASCII.
    pub fn new(encoded: String) -> Result<Self, EvmPrivateKeyImportError> {
        let encoded = Zeroizing::new(encoded);
        if encoded.is_empty() || encoded.len() > MAX_PRIVATE_KEY_INPUT_BYTES || !encoded.is_ascii()
        {
            return Err(EvmPrivateKeyImportError::InvalidInput);
        }
        Ok(Self(encoded))
    }

    pub(super) fn into_encoded(self) -> Zeroizing<String> {
        self.0
    }
}
