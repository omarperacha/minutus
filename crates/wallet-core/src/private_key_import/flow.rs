use crate::EvmAddress;

use super::{EvmPrivateKeyImportError, EvmPrivateKeyInput, ImportedEvmPrivateKey};

/// Required warning shown before an imported private key can be committed.
pub const IMPORTED_KEY_RECOVERY_WARNING: &str = "This imported account is not covered by the app recovery phrase. Recovery requires this private key.";

/// Validated private-key import exposing only its derived public address.
pub struct EvmPrivateKeyImportPreview {
    private_key: ImportedEvmPrivateKey,
    address: EvmAddress,
}

impl EvmPrivateKeyImportPreview {
    /// Validates the encoding and secp256k1 scalar, then derives the EVM address.
    ///
    /// # Errors
    ///
    /// Returns a closed encoding, range or derivation failure without exposing
    /// private-key input or bytes.
    pub fn validate(input: EvmPrivateKeyInput) -> Result<Self, EvmPrivateKeyImportError> {
        let encoded = input.into_encoded();
        let private_key = ImportedEvmPrivateKey::parse(encoded.as_str())?;
        let address = private_key.address()?;
        Ok(Self {
            private_key,
            address,
        })
    }

    /// Returns the locally derived address that must be displayed before import.
    #[must_use]
    pub fn address(&self) -> &EvmAddress {
        &self.address
    }

    /// Returns the warning that must accompany this address preview.
    #[must_use]
    pub const fn recovery_warning(&self) -> &'static str {
        IMPORTED_KEY_RECOVERY_WARNING
    }

    /// Records explicit acknowledgement of imported-key recovery requirements.
    ///
    /// # Errors
    ///
    /// Returns [`EvmPrivateKeyImportError::RecoveryWarningNotAcknowledged`] when
    /// the warning has not been affirmatively acknowledged.
    pub fn acknowledge_recovery_warning(
        &self,
        acknowledged: bool,
    ) -> Result<EvmPrivateKeyRecoveryAcknowledgement, EvmPrivateKeyImportError> {
        if !acknowledged {
            return Err(EvmPrivateKeyImportError::RecoveryWarningNotAcknowledged);
        }
        Ok(EvmPrivateKeyRecoveryAcknowledgement {
            address: self.address.clone(),
        })
    }

    /// Confirms the previewed address and recovery warning for import.
    ///
    /// # Errors
    ///
    /// Returns [`EvmPrivateKeyImportError::StaleAcknowledgement`] when the
    /// acknowledgement was issued for another address preview.
    pub fn confirm(
        self,
        acknowledgement: &EvmPrivateKeyRecoveryAcknowledgement,
    ) -> Result<EvmPrivateKeyImportCommit, EvmPrivateKeyImportError> {
        if acknowledgement.address != self.address {
            return Err(EvmPrivateKeyImportError::StaleAcknowledgement);
        }
        Ok(EvmPrivateKeyImportCommit {
            private_key: self.private_key,
            address: self.address,
        })
    }
}

/// Preview-bound proof that imported-key recovery requirements were acknowledged.
pub struct EvmPrivateKeyRecoveryAcknowledgement {
    address: EvmAddress,
}

/// Private-key import capability issued only after validation and confirmation.
pub struct EvmPrivateKeyImportCommit {
    private_key: ImportedEvmPrivateKey,
    address: EvmAddress,
}

impl EvmPrivateKeyImportCommit {
    /// Returns the confirmed public address.
    #[must_use]
    pub fn address(&self) -> &EvmAddress {
        &self.address
    }

    /// Consumes the capability and releases the key to the encrypted-vault writer.
    #[must_use]
    pub fn into_private_key(self) -> ImportedEvmPrivateKey {
        self.private_key
    }
}
