use zeroize::Zeroizing;

use super::WalletRestoreError;

const MAX_RECOVERY_PHRASE_INPUT_BYTES: usize = 512;

/// Owned, zeroizing recovery-phrase input for local validation.
///
/// This type intentionally implements neither cloning, debugging, formatting
/// nor serialization. It accepts only English BIP-39's lowercase ASCII words
/// and ASCII whitespace, avoiding normalized secret copies.
pub struct RecoveryPhraseInput(Zeroizing<String>);

impl RecoveryPhraseInput {
    /// Takes ownership of bounded recovery-phrase text.
    ///
    /// # Errors
    ///
    /// Returns [`WalletRestoreError::InvalidPhraseInput`] when the text is empty,
    /// oversized or contains bytes outside lowercase ASCII and ASCII whitespace.
    pub fn new(words: String) -> Result<Self, WalletRestoreError> {
        let words = Zeroizing::new(words);
        if words.is_empty()
            || words.len() > MAX_RECOVERY_PHRASE_INPUT_BYTES
            || !words
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_whitespace())
        {
            return Err(WalletRestoreError::InvalidPhraseInput);
        }
        Ok(Self(words))
    }

    pub(super) fn into_words(self) -> Zeroizing<String> {
        self.0
    }
}
