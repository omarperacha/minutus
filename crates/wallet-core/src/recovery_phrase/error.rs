use std::{error::Error, fmt};

/// Closed failure categories for recovery-phrase generation and presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryPhraseError {
    /// The operating system could not provide cryptographic randomness.
    RandomnessUnavailable,
    /// The generated entropy could not be represented as a BIP-39 phrase.
    InvalidEntropy,
    /// The one-time onboarding presentation has already been consumed.
    AlreadyPresented,
    /// Verification cannot begin until the phrase has been presented.
    NotPresented,
    /// A verification challenge is already awaiting an answer.
    VerificationPending,
    /// Recovery-phrase verification has already succeeded.
    AlreadyVerified,
    /// The supplied verification prompt is absent or no longer current.
    StaleVerification,
    /// A submitted verification word violates the bounded input format.
    InvalidVerificationWord,
    /// All recovery warnings were not explicitly acknowledged.
    WarningsNotAcknowledged,
    /// One or more selected recovery words were incorrect.
    IncorrectVerificationWords,
    /// Seed derivation is unavailable until verification succeeds.
    VerificationRequired,
}

impl fmt::Display for RecoveryPhraseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::RandomnessUnavailable => "cryptographic randomness is unavailable",
            Self::InvalidEntropy => "recovery-phrase entropy is invalid",
            Self::AlreadyPresented => "recovery phrase has already been presented",
            Self::NotPresented => "recovery phrase has not been presented",
            Self::VerificationPending => "recovery-phrase verification is already pending",
            Self::AlreadyVerified => "recovery phrase has already been verified",
            Self::StaleVerification => "recovery-phrase verification is not current",
            Self::InvalidVerificationWord => "recovery-phrase verification word is invalid",
            Self::WarningsNotAcknowledged => "recovery warnings were not acknowledged",
            Self::IncorrectVerificationWords => "recovery-phrase verification words are incorrect",
            Self::VerificationRequired => "recovery phrase has not been verified",
        })
    }
}

impl Error for RecoveryPhraseError {}
