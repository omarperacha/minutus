use std::{error::Error, fmt};

use crate::VaultKeyId;

/// Content-free failure from privileged wallet-deletion storage effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VaultDeletionEffectError;

impl fmt::Display for VaultDeletionEffectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("privileged wallet deletion effect failed")
    }
}

impl Error for VaultDeletionEffectError {}

/// Result of removing one class of local wallet material.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultRemovalStatus {
    /// Material existed and was removed.
    Removed,
    /// Material was already absent.
    AlreadyAbsent,
}

/// Honest secure-removal guarantee for encrypted wallet material.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultEncryptedMaterialRemoval {
    /// The device-only DEK is absent and encrypted artifacts were unlinked.
    ///
    /// Physical block overwrite is best effort because copy-on-write filesystems
    /// and flash wear levelling may retain unreachable historical blocks.
    CryptographicErasureWithBestEffortPhysicalRemoval,
}

/// Narrow privileged effects required by a confirmed wallet deletion.
///
/// Implementations must resolve `key_id` only inside the private wallet store.
/// They must never accept a caller-controlled path. Encrypted-material removal
/// covers the live vault and every migration, staging or backup artifact for the
/// same ID, uses the strongest platform removal primitive available and syncs
/// owning directories where supported.
pub trait VaultDeletionEffects {
    /// Removes the device-only Keychain vault key.
    ///
    /// # Errors
    ///
    /// Returns a content-free failure if the key cannot be made absent.
    fn remove_vault_key(
        &mut self,
        key_id: &VaultKeyId,
    ) -> Result<VaultRemovalStatus, VaultDeletionEffectError>;

    /// Removes all encrypted local material for the vault.
    ///
    /// # Errors
    ///
    /// Returns a content-free failure if any owned encrypted artifact remains.
    fn remove_encrypted_material(
        &mut self,
        key_id: &VaultKeyId,
    ) -> Result<VaultRemovalStatus, VaultDeletionEffectError>;
}

/// Completed deletion result without filesystem paths or secret material.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VaultDeletionOutcome {
    key: VaultRemovalStatus,
    encrypted_material: VaultRemovalStatus,
    removal: VaultEncryptedMaterialRemoval,
}

impl VaultDeletionOutcome {
    pub(super) const fn new(
        key: VaultRemovalStatus,
        encrypted_material: VaultRemovalStatus,
    ) -> Self {
        Self {
            key,
            encrypted_material,
            removal:
                VaultEncryptedMaterialRemoval::CryptographicErasureWithBestEffortPhysicalRemoval,
        }
    }

    /// Whether the Keychain item was removed or already absent.
    #[must_use]
    pub const fn key_status(self) -> VaultRemovalStatus {
        self.key
    }

    /// Whether encrypted local artifacts were removed or already absent.
    #[must_use]
    pub const fn encrypted_material_status(self) -> VaultRemovalStatus {
        self.encrypted_material
    }

    /// Returns the strongest guarantee that can be claimed portably.
    #[must_use]
    pub const fn removal_guarantee(self) -> VaultEncryptedMaterialRemoval {
        self.removal
    }
}
