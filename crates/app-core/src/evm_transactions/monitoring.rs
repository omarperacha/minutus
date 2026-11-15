use crate::{AppCore, AppCoreError};
use app_storage::StoredEvmTransaction;

impl AppCore {
    /// Lists pending records that have not received native uncertainty acknowledgement.
    /// # Errors
    /// Returns a storage/corruption failure.
    pub fn unacknowledged_evm_transactions(
        &self,
    ) -> Result<Vec<StoredEvmTransaction>, AppCoreError> {
        self.with_storage(|storage| {
            let mut pending = Vec::new();
            for record in storage.evm_transactions()? {
                if record.state == app_storage::StoredEvmTransactionState::Pending
                    && storage.evm_dispatch_phase(&record.transaction_hash)?
                        != Some(app_storage::StoredEvmDispatchPhase::Acknowledged)
                {
                    pending.push(record);
                }
            }
            Ok(pending)
        })
    }

    /// Records a native user decision to permit a separate new request despite uncertainty.
    /// # Errors
    /// Returns a storage failure or false for a stale/ineligible record.
    pub fn acknowledge_evm_uncertainty(&self, hash: &str) -> Result<bool, AppCoreError> {
        self.with_storage(|storage| storage.acknowledge_evm_uncertainty(hash))
    }

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
