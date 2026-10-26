use bip39::{Language, Mnemonic};

use crate::{EvmAddress, RecoverySeed, VaultKeyId, derive_default_evm_account};

use super::{RecoveryPhraseInput, VaultInventory, WalletRestoreError};

/// Locally derived public EVM address shown before restore confirmation.
pub type RestoredEvmAddress = EvmAddress;

/// Explicit user choice for adding or replacing a local vault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WalletRestoreChoice {
    /// Add a new vault under an identifier absent from the preview snapshot.
    Add {
        /// New identifier that must remain absent through commit authorization.
        new_vault_id: VaultKeyId,
    },
    /// Replace the exact existing vault named by the user.
    Replace {
        /// Existing identifier explicitly selected for replacement.
        existing_vault_id: VaultKeyId,
    },
}

/// Validated restore state exposing only the expected public address and inventory.
///
/// The secret seed is retained privately and cannot be extracted before explicit
/// add-or-replace confirmation and a fresh inventory check.
pub struct WalletRestorePreview {
    seed: RecoverySeed,
    address: RestoredEvmAddress,
    inventory: VaultInventory,
}

impl WalletRestorePreview {
    /// Validates the phrase and checksum locally, then derives the preview address.
    ///
    /// # Errors
    ///
    /// Returns a closed invalid-phrase or derivation failure without exposing
    /// phrase text, word positions, entropy or key material.
    pub fn validate(
        input: RecoveryPhraseInput,
        inventory: VaultInventory,
    ) -> Result<Self, WalletRestoreError> {
        let words = input.into_words();
        let mnemonic = Mnemonic::parse_in_normalized(Language::English, words.as_str())
            .map_err(|_| WalletRestoreError::InvalidRecoveryPhrase)?;
        let seed = RecoverySeed::new(mnemonic.to_seed_normalized(""));
        let address = derive_default_evm_account(&seed)
            .map_err(|_| WalletRestoreError::KeyDerivationFailed)?
            .address()
            .clone();
        Ok(Self {
            seed,
            address,
            inventory,
        })
    }

    /// Returns the locally derived address that must be previewed to the user.
    #[must_use]
    pub fn address(&self) -> &RestoredEvmAddress {
        &self.address
    }

    /// Returns the inventory snapshot against which the choice will be checked.
    #[must_use]
    pub fn inventory(&self) -> &VaultInventory {
        &self.inventory
    }

    /// Records the user's explicit add-or-replace choice.
    ///
    /// # Errors
    ///
    /// Adding fails when the chosen ID already exists. Replacement fails unless
    /// the chosen ID exists in the preview snapshot.
    pub fn confirm(
        self,
        choice: WalletRestoreChoice,
    ) -> Result<ConfirmedWalletRestore, WalletRestoreError> {
        match &choice {
            WalletRestoreChoice::Add { new_vault_id } if self.inventory.contains(new_vault_id) => {
                return Err(WalletRestoreError::AddTargetExists);
            }
            WalletRestoreChoice::Replace { existing_vault_id }
                if !self.inventory.contains(existing_vault_id) =>
            {
                return Err(WalletRestoreError::ReplaceTargetMissing);
            }
            WalletRestoreChoice::Add { .. } | WalletRestoreChoice::Replace { .. } => {}
        }
        Ok(ConfirmedWalletRestore {
            seed: self.seed,
            address: self.address,
            preview_inventory: self.inventory,
            choice,
        })
    }
}

/// Secret-bearing restore state after explicit user confirmation.
pub struct ConfirmedWalletRestore {
    seed: RecoverySeed,
    address: RestoredEvmAddress,
    preview_inventory: VaultInventory,
    choice: WalletRestoreChoice,
}

impl ConfirmedWalletRestore {
    /// Rechecks local state immediately before authorizing a vault mutation.
    ///
    /// # Errors
    ///
    /// Returns [`WalletRestoreError::InventoryChanged`] unless the current
    /// canonical inventory exactly matches the previewed inventory.
    pub fn authorize_commit(
        self,
        current_inventory: &VaultInventory,
    ) -> Result<WalletRestoreCommit, WalletRestoreError> {
        if self.preview_inventory != *current_inventory {
            return Err(WalletRestoreError::InventoryChanged);
        }
        Ok(WalletRestoreCommit {
            seed: self.seed,
            address: self.address,
            choice: self.choice,
        })
    }
}

/// Restore capability issued only after preview, confirmation and state recheck.
///
/// A future vault writer must require this token; there is no implicit default
/// to replacement and no operation that can reinterpret `Add` as `Replace`.
pub struct WalletRestoreCommit {
    seed: RecoverySeed,
    address: RestoredEvmAddress,
    choice: WalletRestoreChoice,
}

impl WalletRestoreCommit {
    /// Returns the confirmed public address.
    #[must_use]
    pub fn address(&self) -> &RestoredEvmAddress {
        &self.address
    }

    /// Returns the exact confirmed add-or-replace operation.
    #[must_use]
    pub fn choice(&self) -> &WalletRestoreChoice {
        &self.choice
    }

    /// Consumes the commit capability and releases its zeroizing seed to the
    /// owning encrypted-vault writer.
    #[must_use]
    pub fn into_seed(self) -> RecoverySeed {
        self.seed
    }
}
