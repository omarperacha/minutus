use crate::VaultKeyId;

use super::WalletRestoreError;

/// Maximum number of local vaults represented in a restore snapshot.
pub const MAX_LOCAL_VAULTS: usize = 64;

/// Canonical non-secret snapshot of local vault identifiers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultInventory(Vec<VaultKeyId>);

impl VaultInventory {
    /// Creates a bounded, duplicate-free inventory in canonical identifier order.
    ///
    /// # Errors
    ///
    /// Returns a limit or duplicate failure for an invalid snapshot.
    pub fn new(mut vault_ids: Vec<VaultKeyId>) -> Result<Self, WalletRestoreError> {
        if vault_ids.len() > MAX_LOCAL_VAULTS {
            return Err(WalletRestoreError::TooManyVaults);
        }
        vault_ids.sort_unstable_by(|left, right| left.as_str().cmp(right.as_str()));
        if vault_ids
            .windows(2)
            .any(|pair| pair[0].as_str() == pair[1].as_str())
        {
            return Err(WalletRestoreError::DuplicateVaultId);
        }
        Ok(Self(vault_ids))
    }

    /// Creates an empty local-vault snapshot.
    #[must_use]
    pub const fn empty() -> Self {
        Self(Vec::new())
    }

    /// Returns the canonical public vault identifiers in this snapshot.
    #[must_use]
    pub fn vault_ids(&self) -> &[VaultKeyId] {
        &self.0
    }

    pub(super) fn contains(&self, vault_id: &VaultKeyId) -> bool {
        self.0
            .binary_search_by(|candidate| candidate.as_str().cmp(vault_id.as_str()))
            .is_ok()
    }
}
