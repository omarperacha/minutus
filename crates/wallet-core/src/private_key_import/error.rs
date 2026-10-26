use std::{error::Error, fmt};

/// Closed failure categories for EVM private-key import.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvmPrivateKeyImportError {
    /// Input is empty, oversized or not ASCII.
    InvalidInput,
    /// Input is not exactly one optionally prefixed 32-byte hexadecimal value.
    InvalidEncoding,
    /// Decoded bytes are zero or outside the secp256k1 scalar range.
    ScalarOutOfRange,
    /// Public-address derivation failed.
    AddressDerivationFailed,
    /// The imported-key recovery warning was not acknowledged.
    RecoveryWarningNotAcknowledged,
    /// The acknowledgement belongs to a different address preview.
    StaleAcknowledgement,
}

impl fmt::Display for EvmPrivateKeyImportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidInput => "private-key input is invalid",
            Self::InvalidEncoding => "private key must be exactly 32 hexadecimal bytes",
            Self::ScalarOutOfRange => "private key is outside the secp256k1 scalar range",
            Self::AddressDerivationFailed => "private-key address derivation failed",
            Self::RecoveryWarningNotAcknowledged => {
                "imported-key recovery warning was not acknowledged"
            }
            Self::StaleAcknowledgement => "private-key acknowledgement is not current",
        })
    }
}

impl Error for EvmPrivateKeyImportError {}
