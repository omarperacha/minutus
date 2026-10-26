use std::{error::Error, fmt, str::FromStr as _};

use bip32::{DerivationPath, XPrv};

use crate::{
    AccountKeyOrigin, AccountMetadata, EvmAddress, RecoverySeed, VaultFormatError,
    evm_address::from_uncompressed_public_key,
};

/// BIP-44 path for the first EVM account derived from the app recovery phrase.
pub const EVM_DEFAULT_DERIVATION_PATH: &str = "m/44'/60'/0'/0/0";

/// Recovery guidance displayed for the default recovery-phrase account.
pub const DEFAULT_EVM_ACCOUNT_RECOVERY_GUIDANCE: &str = "This account is restored from your recovery phrase using EVM derivation path m/44'/60'/0'/0/0.";

/// Public identity of the default recovery-phrase-derived EVM account.
///
/// The extended and leaf private keys exist only during derivation and are
/// dropped before this value is returned. This type intentionally contains only
/// the address and the fixed public recovery path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultEvmAccount {
    address: EvmAddress,
}

impl DefaultEvmAccount {
    /// Returns the canonical lowercase EVM address.
    #[must_use]
    pub const fn address(&self) -> &EvmAddress {
        &self.address
    }

    /// Returns the fixed BIP-44 recovery path for this account.
    #[must_use]
    pub const fn derivation_path(&self) -> &'static str {
        EVM_DEFAULT_DERIVATION_PATH
    }

    /// Constructs authenticated, non-secret vault metadata for one chain.
    ///
    /// # Errors
    ///
    /// Returns [`VaultFormatError`] when the account identifier or chain ID is
    /// invalid. The address and fixed derivation path are already canonical.
    pub fn metadata(
        &self,
        account_id: impl Into<String>,
        chain_id: u64,
    ) -> Result<AccountMetadata, VaultFormatError> {
        AccountMetadata::new(
            account_id,
            self.address.as_str(),
            chain_id,
            AccountKeyOrigin::recovery_phrase(EVM_DEFAULT_DERIVATION_PATH)?,
        )
    }
}

/// Closed failure categories for deterministic EVM account derivation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvmAccountDerivationError {
    /// The application-owned path could not be parsed as BIP-32.
    InvalidDerivationPath,
    /// BIP-32 or secp256k1 private-key derivation failed.
    KeyDerivationFailed,
    /// The derived public key could not be converted to an EVM address.
    AddressDerivationFailed,
}

impl fmt::Display for EvmAccountDerivationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidDerivationPath => "default EVM derivation path is invalid",
            Self::KeyDerivationFailed => "default EVM account key derivation failed",
            Self::AddressDerivationFailed => "default EVM address derivation failed",
        })
    }
}

impl Error for EvmAccountDerivationError {}

/// Derives the default EVM account's public identity from a verified recovery seed.
///
/// Derivation uses BIP-32/secp256k1 at [`EVM_DEFAULT_DERIVATION_PATH`]. Secret
/// seed and private-key bytes remain inside `wallet-core`; only the public
/// address and fixed recovery metadata are returned.
///
/// # Errors
///
/// Returns a closed error when the fixed path, child-key derivation or public
/// address conversion fails.
pub fn derive_default_evm_account(
    seed: &RecoverySeed,
) -> Result<DefaultEvmAccount, EvmAccountDerivationError> {
    let path = DerivationPath::from_str(EVM_DEFAULT_DERIVATION_PATH)
        .map_err(|_| EvmAccountDerivationError::InvalidDerivationPath)?;
    let extended_private_key = XPrv::derive_from_path(seed.expose_secret(), &path)
        .map_err(|_| EvmAccountDerivationError::KeyDerivationFailed)?;
    let encoded_public_key = extended_private_key
        .private_key()
        .verifying_key()
        .to_sec1_point(false);
    let address = from_uncompressed_public_key(encoded_public_key.as_bytes())
        .ok_or(EvmAccountDerivationError::AddressDerivationFailed)?;
    Ok(DefaultEvmAccount { address })
}

#[cfg(test)]
mod tests;
