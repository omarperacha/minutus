//! Restart-safe persistence of public EVM transaction monitoring metadata.

use app_storage::StoredEvmTransaction;

use crate::{AppCore, AppCoreError};

impl AppCore {
    /// Persists one node-accepted transaction or later chain observation.
    ///
    /// # Errors
    ///
    /// Returns an error when metadata is malformed or storage is unavailable.
    pub fn save_evm_transaction(
        &self,
        transaction: &StoredEvmTransaction,
    ) -> Result<(), AppCoreError> {
        self.with_storage(|storage| storage.save_evm_transaction(transaction))
    }

    /// Loads all non-secret EVM monitoring records after restart.
    ///
    /// # Errors
    ///
    /// Returns an error when storage is unavailable or persisted data is invalid.
    pub fn evm_transactions(&self) -> Result<Vec<StoredEvmTransaction>, AppCoreError> {
        self.with_storage(|storage| storage.evm_transactions())
    }
}
