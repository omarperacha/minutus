use bip39::{Language, Mnemonic};
use zeroize::Zeroizing;

use super::{RecoveryPhraseError, verification::PendingVerification};

/// Entropy used for a 24-word BIP-39 recovery phrase.
pub const RECOVERY_ENTROPY_BYTES: usize = 32;

/// Number of words in a newly generated recovery phrase.
pub const RECOVERY_PHRASE_WORDS: usize = 24;

/// Secret BIP-39 phrase with one-shot onboarding presentation.
///
/// This type intentionally implements neither `Clone`, `Debug`, `Display` nor
/// serialization. Phrase text is available only inside
/// [`Self::present_once_for_onboarding`]; its temporary UTF-8 buffer is zeroized
/// as soon as the callback returns or unwinds.
pub struct RecoveryPhrase {
    pub(super) mnemonic: Mnemonic,
    pub(super) presented: bool,
    pub(super) pending_verification: Option<PendingVerification>,
    pub(super) verified: bool,
}

impl RecoveryPhrase {
    /// Generates a 24-word English BIP-39 phrase from operating-system entropy.
    ///
    /// # Errors
    ///
    /// Returns [`RecoveryPhraseError::RandomnessUnavailable`] if the operating
    /// system cannot fill the complete entropy buffer. An internal BIP-39
    /// construction failure returns [`RecoveryPhraseError::InvalidEntropy`].
    pub fn generate() -> Result<Self, RecoveryPhraseError> {
        let mut entropy = Zeroizing::new([0_u8; RECOVERY_ENTROPY_BYTES]);
        getrandom::fill(entropy.as_mut())
            .map_err(|_| RecoveryPhraseError::RandomnessUnavailable)?;
        Self::from_entropy(entropy.as_ref())
    }

    /// Presents the phrase once to the trusted native onboarding surface.
    ///
    /// The callback must render the borrowed text directly. It must not log,
    /// persist, copy into frontend state or otherwise retain the phrase. The
    /// presentation is marked consumed before the callback begins, including
    /// when the callback unwinds.
    ///
    /// # Errors
    ///
    /// Returns [`RecoveryPhraseError::AlreadyPresented`] after the first call.
    pub fn present_once_for_onboarding<T>(
        &mut self,
        present: impl FnOnce(&str) -> T,
    ) -> Result<T, RecoveryPhraseError> {
        if self.presented {
            return Err(RecoveryPhraseError::AlreadyPresented);
        }
        self.presented = true;

        let words = Zeroizing::new(self.mnemonic.to_string());
        Ok(present(words.as_str()))
    }

    fn from_entropy(entropy: &[u8]) -> Result<Self, RecoveryPhraseError> {
        let mnemonic = Mnemonic::from_entropy_in(Language::English, entropy)
            .map_err(|_| RecoveryPhraseError::InvalidEntropy)?;
        debug_assert_eq!(mnemonic.word_count(), RECOVERY_PHRASE_WORDS);
        Ok(Self {
            mnemonic,
            presented: false,
            pending_verification: None,
            verified: false,
        })
    }

    #[cfg(test)]
    pub(super) fn from_test_entropy(
        entropy: &[u8; RECOVERY_ENTROPY_BYTES],
    ) -> Result<Self, RecoveryPhraseError> {
        Self::from_entropy(entropy)
    }
}
