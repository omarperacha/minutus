use std::{error::Error, fmt};

/// Closed failure categories for recovery-phrase restoration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalletRestoreError {
    /// Recovery-phrase input is empty, oversized or contains unsupported bytes.
    InvalidPhraseInput,
    /// The phrase words or BIP-39 checksum are invalid.
    InvalidRecoveryPhrase,
    /// Local BIP-32 or secp256k1 derivation failed.
    KeyDerivationFailed,
    /// The local vault inventory exceeds its reviewed bound.
    TooManyVaults,
    /// The local vault inventory contains a duplicate identifier.
    DuplicateVaultId,
    /// An add operation targets an identifier that already exists.
    AddTargetExists,
    /// A replace operation does not name an existing vault.
    ReplaceTargetMissing,
    /// Local vault state changed after the restore preview.
    InventoryChanged,
}

impl fmt::Display for WalletRestoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidPhraseInput => "recovery-phrase input is invalid",
            Self::InvalidRecoveryPhrase => "recovery phrase or checksum is invalid",
            Self::KeyDerivationFailed => "wallet account derivation failed",
            Self::TooManyVaults => "local vault inventory exceeds its limit",
            Self::DuplicateVaultId => "local vault inventory contains a duplicate",
            Self::AddTargetExists => "new vault identifier already exists",
            Self::ReplaceTargetMissing => "replacement vault was not found",
            Self::InventoryChanged => "local vault inventory changed after preview",
        })
    }
}

impl Error for WalletRestoreError {}
