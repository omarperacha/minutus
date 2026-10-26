//! BIP-39 recovery-phrase generation and bounded onboarding presentation.

mod error;
mod phrase;
mod seed;
mod verification;

pub use error::RecoveryPhraseError;
pub use phrase::{RECOVERY_ENTROPY_BYTES, RECOVERY_PHRASE_WORDS, RecoveryPhrase};
pub use seed::RecoverySeed;
pub use verification::{
    RECOVERY_VERIFICATION_WARNINGS, RECOVERY_VERIFICATION_WORDS, RecoveryVerificationPrompt,
    RecoveryVerificationWarnings, RecoveryWarningAcknowledgement, RecoveryWordAnswers,
    RecoveryWordPosition,
};

#[cfg(test)]
mod tests;
