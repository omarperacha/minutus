use std::{error::Error, fmt};

/// Closed failure categories for safeguarded wallet deletion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultDeletionError {
    /// Confirmation was issued for a different key-protection/deletion contract.
    WrongProtectionMode,
    /// Typed confirmation input is empty, oversized or not printable ASCII.
    InvalidTypedConfirmation,
    /// The destructive warning was not explicitly acknowledged.
    DestructiveWarningNotAcknowledged,
    /// Typed confirmation did not exactly match the vault-bound prompt.
    TypedConfirmationMismatch,
    /// The supplied live session belongs to a different vault.
    SessionForDifferentVault,
    /// The device-only vault key could not be removed, so deletion stopped.
    KeyRemovalFailed,
    /// The key was removed but encrypted local artifacts could not all be removed.
    EncryptedMaterialRemovalFailedAfterKeyRemoval,
}

impl fmt::Display for VaultDeletionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::WrongProtectionMode => "wallet deletion protection mode does not match",
            Self::InvalidTypedConfirmation => "wallet deletion confirmation is invalid",
            Self::DestructiveWarningNotAcknowledged => {
                "wallet deletion warning was not acknowledged"
            }
            Self::TypedConfirmationMismatch => "wallet deletion confirmation does not match",
            Self::SessionForDifferentVault => "wallet deletion session belongs to another vault",
            Self::KeyRemovalFailed => "wallet key removal failed before vault deletion",
            Self::EncryptedMaterialRemovalFailedAfterKeyRemoval => {
                "encrypted wallet material remains after key removal"
            }
        })
    }
}

impl Error for VaultDeletionError {}
