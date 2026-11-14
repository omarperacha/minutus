use crate::{AppCore, AppCoreError};
use app_storage::EvmNonceClaim;
use app_storage::StoredEvmTransaction;

impl AppCore {
    /// Atomically reserves a new hash before dispatch.
    /// # Errors
    /// Returns storage/duplicate errors; the caller must not send.
    pub fn reserve_evm_transaction(
        &self,
        record: &StoredEvmTransaction,
        claim: &EvmNonceClaim,
    ) -> Result<(), AppCoreError> {
        self.with_storage(|storage| storage.reserve_evm_transaction(record, claim))
    }

    /// Durably advances a reservation exactly once before final transport admission.
    /// # Errors
    /// Returns a storage error; false never permits a send.
    pub fn mark_evm_dispatched(
        &self,
        hash: &str,
        claim: &EvmNonceClaim,
    ) -> Result<bool, AppCoreError> {
        self.with_storage(|storage| storage.mark_evm_dispatched(hash, claim))
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

    /// Retires a monitoring reservation after final host admission rejects a known-unsent transaction.
    /// # Errors
    /// Returns a storage failure; a failed cleanup leaves conservative monitoring metadata.
    pub fn discard_unsent_evm_transaction(
        &self,
        hash: &str,
        claim: &EvmNonceClaim,
    ) -> Result<(), AppCoreError> {
        self.with_storage(|storage| storage.discard_unsent_evm_transaction(hash, claim))
    }
}
