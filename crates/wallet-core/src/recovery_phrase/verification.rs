use zeroize::Zeroizing;

use super::{RECOVERY_PHRASE_WORDS, RecoveryPhrase, RecoveryPhraseError, RecoverySeed};

/// Number of randomly selected words required for recovery verification.
pub const RECOVERY_VERIFICATION_WORDS: usize = 3;

const VERIFICATION_ID_BYTES: usize = 16;
const MAX_VERIFICATION_WORD_BYTES: usize = 16;

/// Required warnings shown during recovery-phrase verification.
pub const RECOVERY_VERIFICATION_WARNINGS: RecoveryVerificationWarnings =
    RecoveryVerificationWarnings {
        no_cloud_backup: "There is no cloud backup for this wallet.",
        device_and_phrase_loss: "If you lose both this device and your recovery phrase, access to the wallet is permanently lost.",
        support_cannot_recover: "Support cannot recover this wallet or its recovery phrase.",
    };

/// Fixed, host-owned warning copy for recovery-phrase verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryVerificationWarnings {
    no_cloud_backup: &'static str,
    device_and_phrase_loss: &'static str,
    support_cannot_recover: &'static str,
}

impl RecoveryVerificationWarnings {
    /// Warns that the wallet has no cloud backup.
    #[must_use]
    pub const fn no_cloud_backup(self) -> &'static str {
        self.no_cloud_backup
    }

    /// Warns that losing both recovery sources permanently loses access.
    #[must_use]
    pub const fn device_and_phrase_loss(self) -> &'static str {
        self.device_and_phrase_loss
    }

    /// Warns that support cannot recover the wallet.
    #[must_use]
    pub const fn support_cannot_recover(self) -> &'static str {
        self.support_cannot_recover
    }
}

/// One-based position of a word selected for verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct RecoveryWordPosition(usize);

impl RecoveryWordPosition {
    /// Returns the one-based word number shown to the user.
    #[must_use]
    pub const fn number(self) -> usize {
        self.0 + 1
    }

    const fn index(self) -> usize {
        self.0
    }
}

/// Non-secret verification prompt containing random word positions and warnings.
///
/// The opaque prompt is bound to the pending challenge and cannot be
/// constructed, cloned or serialized by a caller.
pub struct RecoveryVerificationPrompt {
    id: [u8; VERIFICATION_ID_BYTES],
    positions: [RecoveryWordPosition; RECOVERY_VERIFICATION_WORDS],
}

impl RecoveryVerificationPrompt {
    /// Returns the randomly selected positions in ascending display order.
    #[must_use]
    pub const fn positions(&self) -> [RecoveryWordPosition; RECOVERY_VERIFICATION_WORDS] {
        self.positions
    }

    /// Returns the complete warning copy that must accompany this prompt.
    #[must_use]
    pub const fn warnings(&self) -> RecoveryVerificationWarnings {
        RECOVERY_VERIFICATION_WARNINGS
    }

    /// Records explicit acknowledgement of all recovery warnings.
    ///
    /// # Errors
    ///
    /// Returns [`RecoveryPhraseError::WarningsNotAcknowledged`] unless every
    /// warning has been affirmatively acknowledged.
    pub const fn acknowledge_warnings(
        &self,
        no_cloud_backup: bool,
        device_and_phrase_loss: bool,
        support_cannot_recover: bool,
    ) -> Result<RecoveryWarningAcknowledgement, RecoveryPhraseError> {
        if !(no_cloud_backup && device_and_phrase_loss && support_cannot_recover) {
            return Err(RecoveryPhraseError::WarningsNotAcknowledged);
        }
        Ok(RecoveryWarningAcknowledgement { prompt_id: self.id })
    }
}

/// Prompt-bound proof that all recovery warnings were acknowledged.
pub struct RecoveryWarningAcknowledgement {
    prompt_id: [u8; VERIFICATION_ID_BYTES],
}

/// Owned, zeroizing answers for one recovery verification attempt.
///
/// This type intentionally implements neither cloning, debugging, formatting
/// nor serialization. Input strings are zeroized on both success and failure.
pub struct RecoveryWordAnswers([Zeroizing<String>; RECOVERY_VERIFICATION_WORDS]);

impl RecoveryWordAnswers {
    /// Takes ownership of three bounded lowercase English-word inputs.
    ///
    /// # Errors
    ///
    /// Returns [`RecoveryPhraseError::InvalidVerificationWord`] when any input
    /// is empty, oversized or contains a character outside lowercase ASCII.
    pub fn new(words: [String; RECOVERY_VERIFICATION_WORDS]) -> Result<Self, RecoveryPhraseError> {
        let words = words.map(Zeroizing::new);
        if words.iter().any(|word| {
            word.is_empty()
                || word.len() > MAX_VERIFICATION_WORD_BYTES
                || !word.bytes().all(|byte| byte.is_ascii_lowercase())
        }) {
            return Err(RecoveryPhraseError::InvalidVerificationWord);
        }
        Ok(Self(words))
    }
}

pub(super) struct PendingVerification {
    id: [u8; VERIFICATION_ID_BYTES],
    positions: [RecoveryWordPosition; RECOVERY_VERIFICATION_WORDS],
}

impl RecoveryPhrase {
    /// Selects three distinct random positions for recovery-phrase verification.
    ///
    /// This is available only after the one-time onboarding presentation. Only
    /// non-secret positions and fixed warning copy are exposed by the prompt.
    ///
    /// # Errors
    ///
    /// Returns a closed state error when presentation has not occurred, a
    /// challenge is already pending or verification already succeeded. Returns
    /// [`RecoveryPhraseError::RandomnessUnavailable`] when the operating system
    /// cannot provide the required random bytes.
    pub fn begin_verification(
        &mut self,
    ) -> Result<RecoveryVerificationPrompt, RecoveryPhraseError> {
        if !self.presented {
            return Err(RecoveryPhraseError::NotPresented);
        }
        if self.verified {
            return Err(RecoveryPhraseError::AlreadyVerified);
        }
        if self.pending_verification.is_some() {
            return Err(RecoveryPhraseError::VerificationPending);
        }

        let positions = select_random_positions()?;
        let mut id = [0_u8; VERIFICATION_ID_BYTES];
        getrandom::fill(&mut id).map_err(|_| RecoveryPhraseError::RandomnessUnavailable)?;
        self.pending_verification = Some(PendingVerification { id, positions });
        Ok(RecoveryVerificationPrompt { id, positions })
    }

    /// Verifies one prompt-bound set of answers and unlocks seed derivation.
    ///
    /// Submitted words are zeroized when this call returns. An incorrect attempt
    /// consumes the pending challenge so another attempt receives fresh random
    /// positions.
    ///
    /// # Errors
    ///
    /// Returns a closed state, acknowledgement or incorrect-word failure. No
    /// error identifies which submitted word was wrong.
    pub fn verify_recovery_words(
        &mut self,
        prompt: &RecoveryVerificationPrompt,
        acknowledgement: &RecoveryWarningAcknowledgement,
        answers: RecoveryWordAnswers,
    ) -> Result<(), RecoveryPhraseError> {
        let RecoveryWordAnswers(answers) = answers;
        let pending = self
            .pending_verification
            .as_ref()
            .ok_or(RecoveryPhraseError::StaleVerification)?;
        if pending.id != prompt.id || pending.positions != prompt.positions {
            return Err(RecoveryPhraseError::StaleVerification);
        }
        if acknowledgement.prompt_id != prompt.id {
            return Err(RecoveryPhraseError::WarningsNotAcknowledged);
        }

        let correct = prompt
            .positions
            .iter()
            .zip(answers.iter())
            .all(|(position, answer)| {
                self.mnemonic.words().nth(position.index()) == Some(answer.as_str())
            });
        self.pending_verification = None;
        if !correct {
            return Err(RecoveryPhraseError::IncorrectVerificationWords);
        }

        self.verified = true;
        Ok(())
    }

    /// Derives the BIP-39 seed after successful word verification.
    ///
    /// # Errors
    ///
    /// Returns [`RecoveryPhraseError::VerificationRequired`] until the current
    /// phrase passes a prompt-bound verification challenge.
    pub fn derive_seed(&self) -> Result<RecoverySeed, RecoveryPhraseError> {
        if !self.verified {
            return Err(RecoveryPhraseError::VerificationRequired);
        }
        Ok(RecoverySeed::new(self.mnemonic.to_seed_normalized("")))
    }
}

fn select_random_positions()
-> Result<[RecoveryWordPosition; RECOVERY_VERIFICATION_WORDS], RecoveryPhraseError> {
    let mut candidates = std::array::from_fn::<_, RECOVERY_PHRASE_WORDS, _>(|index| index);
    for selected in 0..RECOVERY_VERIFICATION_WORDS {
        let remaining = RECOVERY_PHRASE_WORDS - selected;
        let offset = uniform_index(remaining)?;
        candidates.swap(selected, selected + offset);
    }

    let mut positions = std::array::from_fn(|index| RecoveryWordPosition(candidates[index]));
    positions.sort_unstable();
    Ok(positions)
}

fn uniform_index(upper_bound: usize) -> Result<usize, RecoveryPhraseError> {
    let acceptance_limit = 256_usize - (256_usize % upper_bound);
    loop {
        let mut random = [0_u8; 1];
        getrandom::fill(&mut random).map_err(|_| RecoveryPhraseError::RandomnessUnavailable)?;
        let random = usize::from(random[0]);
        if random < acceptance_limit {
            return Ok(random % upper_bound);
        }
    }
}
