//! Atomic public reservation/dispatch/reconciliation facts without signing authority.
use super::{ApplicationStore, OptionalExtension, StorageError, TransactionBehavior, params};
use crate::{StoredEvmDispatchPhase, StoredEvmTransaction, StoredEvmTransactionState};
impl ApplicationStore {
    /// Reserves one new exact hash and its monitoring record atomically.
    /// # Errors
    /// Rejects an already known hash or a failed durable write; no send is allowed.
    pub fn reserve_evm_transaction(
        &mut self,
        record: &StoredEvmTransaction,
        reservation_id: &str,
    ) -> Result<(), StorageError> {
        if reservation_id.is_empty()
            || reservation_id.len() > 160
            || reservation_id
                .bytes()
                .any(|c| !c.is_ascii_alphanumeric() && c != b'-')
        {
            return Err(StorageError::InvalidRecord("invalid reservation identity"));
        }
        if record.state != StoredEvmTransactionState::Pending {
            return Err(StorageError::InvalidRecord(
                "only pending candidates can be reserved",
            ));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let known = transaction
            .query_row(
                "SELECT 1 FROM evm_transactions WHERE transaction_hash=?1",
                [&record.transaction_hash],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if known {
            return Err(StorageError::InvalidRecord(
                "transaction hash is already retained; reconcile it instead of resending",
            ));
        }
        super::evm::write_evm_transaction(&transaction, record)?;
        transaction.execute(
            "INSERT INTO evm_transfer_dispatches (transaction_hash,reservation_id,phase) VALUES (?1,?2,'reserved')",
            params![record.transaction_hash,reservation_id],
        )?;
        transaction.commit()?;
        Ok(())
    }
    /// Advances only a live reservation before the one irreversible send.
    /// # Errors
    /// Returns a storage failure. False means retired, already dispatched or unknown.
    pub fn mark_evm_dispatched(
        &self,
        hash: &str,
        reservation_id: &str,
    ) -> Result<bool, StorageError> {
        Ok(self.connection.execute("UPDATE evm_transfer_dispatches SET phase='dispatched' WHERE transaction_hash=?1 AND reservation_id=?2 AND phase='reserved'",params![hash,reservation_id])? == 1)
    }
    /// Reads public dispatch facts. Legacy pending records have no proven phase.
    /// # Errors
    /// Returns a storage/corruption failure.
    pub fn evm_dispatch_phase(
        &self,
        hash: &str,
    ) -> Result<Option<StoredEvmDispatchPhase>, StorageError> {
        self.connection
            .query_row(
                "SELECT phase FROM evm_transfer_dispatches WHERE transaction_hash=?1",
                [hash],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|value| match value.as_str() {
                "reserved" => Ok(StoredEvmDispatchPhase::Reserved),
                "dispatched" => Ok(StoredEvmDispatchPhase::Dispatched),
                "acknowledged" => Ok(StoredEvmDispatchPhase::Acknowledged),
                _ => Err(StorageError::CorruptDatabase),
            })
            .transpose()
    }
    /// Atomically retires only known-unsent reservations for this account/network.
    /// A competing worker's subsequent reserved-to-dispatched CAS must fail.
    /// # Errors
    /// Returns a storage failure; uncertain/legacy records are never deleted here.
    pub fn retire_evm_reservations(
        &self,
        network: &str,
        sender: &str,
    ) -> Result<usize, StorageError> {
        Ok(self.connection.execute("DELETE FROM evm_transactions WHERE network=?1 AND lower(sender)=lower(?2) AND state='pending' AND transaction_hash IN (SELECT transaction_hash FROM evm_transfer_dispatches WHERE phase='reserved')",params![network,sender])?)
    }
    /// Retires a reservation only when the active worker proves no send occurred.
    /// # Errors
    /// Returns a storage failure; never call after transport admission succeeded.
    pub fn discard_unsent_evm_transaction(
        &self,
        hash: &str,
        reservation_id: &str,
    ) -> Result<(), StorageError> {
        self.connection.execute("DELETE FROM evm_transactions WHERE transaction_hash=?1 AND state='pending' AND transaction_hash IN (SELECT transaction_hash FROM evm_transfer_dispatches WHERE reservation_id=?2 AND phase IN ('reserved','dispatched'))",params![hash,reservation_id])?;
        Ok(())
    }
    /// Records a native acknowledgement for a still-pending, non-reserved exact hash.
    /// It grants no signing authority and never changes the chain-derived status.
    /// # Errors
    /// Returns a storage failure. False means the snapshot is no longer eligible.
    pub fn acknowledge_evm_uncertainty(&self, hash: &str) -> Result<bool, StorageError> {
        Ok(self.connection.execute("INSERT INTO evm_transfer_dispatches (transaction_hash,reservation_id,phase)
            SELECT transaction_hash,'native-acknowledgement','acknowledged' FROM evm_transactions WHERE transaction_hash=?1 AND state='pending'
            ON CONFLICT(transaction_hash) DO UPDATE SET phase='acknowledged' WHERE evm_transfer_dispatches.phase='dispatched'",[hash])? == 1)
    }
}
#[cfg(test)]
mod tests;
