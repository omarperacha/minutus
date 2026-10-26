use crate::{VaultKeyId, VaultLockReason, VaultSession, VaultSessionStatus};

use super::{
    VaultDeletionEffects, VaultDeletionError, VaultDeletionOutcome, VaultTypedConfirmation,
};

/// Required destructive warning shown before wallet deletion.
pub const VAULT_DELETION_WARNING: &str = "Deleting this wallet permanently removes its local encrypted vault and device-only key. This action cannot be undone.";

/// Explicit statement about recovery material made before deletion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultRecoveryConfirmation {
    /// Recovery material for every account in the vault is available.
    AllRecoveryMaterialAvailable,
    /// Recovery material is unavailable and permanent loss is accepted.
    RecoveryUnavailableAndPermanentLossAccepted,
}

/// Non-secret deletion prompt bound to one exact vault identifier.
pub struct VaultDeletionPreview {
    key_id: VaultKeyId,
    required_confirmation: String,
    password_protected: bool,
}

impl VaultDeletionPreview {
    /// Creates the warning and typed-confirmation prompt for one vault.
    #[must_use]
    pub fn new(key_id: VaultKeyId) -> Self {
        let required_confirmation = format!("DELETE {}", key_id.as_str());
        Self {
            key_id,
            required_confirmation,
            password_protected: false,
        }
    }

    /// Returns the explicit destructive warning.
    #[must_use]
    pub const fn warning(&self) -> &'static str {
        if self.password_protected {
            return "Deleting this password wallet removes the local encrypted file. External copies and recovery material can still restore it. This cannot be undone.";
        }
        VAULT_DELETION_WARNING
    }

    /// Creates the same typed/recovery confirmation gate with an honest password
    /// wallet warning, without implying removal of a separate device-only key.
    #[must_use]
    pub fn new_password_protected(key_id: VaultKeyId) -> Self {
        let mut preview = Self::new(key_id);
        preview.password_protected = true;
        preview
    }

    /// Returns the exact vault-bound text the user must enter.
    #[must_use]
    pub fn required_confirmation(&self) -> &str {
        &self.required_confirmation
    }

    /// Applies recovery, warning and typed-confirmation safeguards.
    ///
    /// The recovery value must be selected explicitly. Its unavailable variant
    /// is itself an acknowledgement of permanent loss.
    ///
    /// # Errors
    ///
    /// Returns a closed error unless the warning is acknowledged and the typed
    /// confirmation exactly matches this vault's prompt.
    pub fn confirm(
        self,
        _recovery: VaultRecoveryConfirmation,
        destructive_warning_acknowledged: bool,
        typed_confirmation: VaultTypedConfirmation,
    ) -> Result<VaultDeletionCommit, VaultDeletionError> {
        if !destructive_warning_acknowledged {
            return Err(VaultDeletionError::DestructiveWarningNotAcknowledged);
        }
        let typed_confirmation = typed_confirmation.into_value();
        if typed_confirmation.as_str() != self.required_confirmation {
            return Err(VaultDeletionError::TypedConfirmationMismatch);
        }
        Ok(VaultDeletionCommit {
            key_id: self.key_id,
            password_protected: self.password_protected,
        })
    }
}

/// One-use capability issued only after every deletion safeguard succeeds.
pub struct VaultDeletionCommit {
    key_id: VaultKeyId,
    password_protected: bool,
}

impl VaultDeletionCommit {
    pub(crate) fn authorize_password_removal(
        self,
        session: &mut VaultSession,
    ) -> Result<VaultKeyId, VaultDeletionError> {
        if !self.password_protected {
            return Err(VaultDeletionError::WrongProtectionMode);
        }
        if session.key_id() != &self.key_id {
            return Err(VaultDeletionError::SessionForDifferentVault);
        }
        session.lock_for_deletion();
        Ok(self.key_id)
    }
    /// Executes deletion in fail-closed cryptographic-erasure order.
    ///
    /// The matching live session is locked first. The device-only key is then
    /// removed before encrypted artifacts. A key-removal failure leaves the
    /// encrypted vault untouched. An encrypted-material failure is reported as
    /// partial deletion after the key has already become unavailable.
    ///
    /// # Errors
    ///
    /// Returns a closed wrong-vault or privileged-effect failure.
    pub fn execute(
        self,
        session: &mut VaultSession,
        effects: &mut impl VaultDeletionEffects,
    ) -> Result<VaultDeletionOutcome, VaultDeletionError> {
        if self.password_protected {
            return Err(VaultDeletionError::WrongProtectionMode);
        }
        if session.key_id() != &self.key_id {
            return Err(VaultDeletionError::SessionForDifferentVault);
        }
        session.lock_for_deletion();
        debug_assert_eq!(
            session.status(),
            VaultSessionStatus::Locked(VaultLockReason::Deletion)
        );

        let key = effects
            .remove_vault_key(&self.key_id)
            .map_err(|_| VaultDeletionError::KeyRemovalFailed)?;
        let encrypted_material = effects
            .remove_encrypted_material(&self.key_id)
            .map_err(|_| VaultDeletionError::EncryptedMaterialRemovalFailedAfterKeyRemoval)?;
        Ok(VaultDeletionOutcome::new(key, encrypted_material))
    }
}
