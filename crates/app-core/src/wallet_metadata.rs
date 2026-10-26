//! Persistence coordination for public wallet metadata only.

use app_storage::PublicWalletMetadata;
use ipc_types::WalletId;

use crate::{AppCore, AppCoreError};

impl AppCore {
    /// Persists public selected-wallet state without touching encrypted material.
    ///
    /// # Errors
    ///
    /// Returns a sanitized application-storage failure.
    pub fn save_public_wallet(&self, wallet: &PublicWalletMetadata) -> Result<(), AppCoreError> {
        self.with_storage(|storage| storage.save_public_wallet(wallet))
    }

    /// Removes exact display metadata for an explicitly removed account.
    /// # Errors
    /// Returns a sanitised application-storage failure.
    pub fn remove_public_wallet(&self, wallet_id: &WalletId) -> Result<(), AppCoreError> {
        self.with_storage(|storage| storage.remove_public_wallet(wallet_id))
    }

    /// Loads public state for one exact opaque wallet identifier.
    ///
    /// # Errors
    ///
    /// Returns a sanitized application-storage failure.
    pub fn public_wallet(
        &self,
        wallet_id: &WalletId,
    ) -> Result<Option<PublicWalletMetadata>, AppCoreError> {
        self.with_storage(|storage| storage.public_wallet(wallet_id))
    }

    /// Loads the most recently selected public wallet without secret access.
    ///
    /// # Errors
    ///
    /// Returns a sanitized application-storage failure.
    pub fn latest_public_wallet(&self) -> Result<Option<PublicWalletMetadata>, AppCoreError> {
        self.with_storage(|storage| storage.latest_public_wallet())
    }
}
