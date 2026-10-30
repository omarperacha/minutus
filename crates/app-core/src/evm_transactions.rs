//! Restart-safe persistence of public EVM transaction monitoring metadata.

use app_storage::StoredEvmTransaction;

use crate::{AppCore, AppCoreError};

impl AppCore {
    /// Atomically reserves a new hash before dispatch.
    /// # Errors
    /// Returns storage/duplicate errors; the caller must not send.
    pub fn reserve_evm_transaction(
        &self,
        record: &StoredEvmTransaction,
        reservation_id: &str,
    ) -> Result<(), AppCoreError> {
        self.with_storage(|storage| storage.reserve_evm_transaction(record, reservation_id))
    }
    /// Durably advances a reservation exactly once before final transport admission.
    /// # Errors
    /// Returns a storage error; false never permits a send.
    pub fn mark_evm_dispatched(
        &self,
        hash: &str,
        reservation_id: &str,
    ) -> Result<bool, AppCoreError> {
        self.with_storage(|storage| storage.mark_evm_dispatched(hash, reservation_id))
    }
    /// Retires only atomically proven-unsent work for the selected scope.
    /// # Errors
    /// Returns a storage failure; unknown sends are retained.
    pub fn retire_evm_reservations(
        &self,
        network: &str,
        sender: &str,
    ) -> Result<usize, AppCoreError> {
        self.with_storage(|storage| storage.retire_evm_reservations(network, sender))
    }
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

    /// Retires a monitoring reservation after final host admission rejects a known-unsent transaction.
    /// # Errors
    /// Returns a storage failure; a failed cleanup leaves conservative monitoring metadata.
    pub fn discard_unsent_evm_transaction(
        &self,
        hash: &str,
        reservation_id: &str,
    ) -> Result<(), AppCoreError> {
        self.with_storage(|storage| storage.discard_unsent_evm_transaction(hash, reservation_id))
    }

    /// Durably admits at most one transfer per persisted user turn, including retries.
    /// # Errors
    /// Returns a storage failure; no transaction may proceed in that case.
    pub fn claim_chat_transfer(
        &self,
        conversation: &ipc_types::ConversationId,
        message: &ipc_types::MessageId,
    ) -> Result<bool, AppCoreError> {
        self.with_storage(|storage| storage.claim_chat_transfer(conversation, message))
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
