//! Atomic public reservation/dispatch/reconciliation facts without signing authority.
use super::{ApplicationStore, OptionalExtension, StorageError, TransactionBehavior, params};
use crate::{
    EvmNonceClaim, StoredEvmDispatchPhase, StoredEvmTransaction, StoredEvmTransactionState,
};
impl ApplicationStore {
    /// Binds one signed hash and monitoring record to an exact pre-auth nonce owner.
    /// # Errors
    /// Rejects changed/retired owners, an already-bound hash or durable failure.
    pub fn reserve_evm_transaction(
        &mut self,
        record: &StoredEvmTransaction,
        claim: &EvmNonceClaim,
    ) -> Result<(), StorageError> {
        if !claim.matches(record) {
            return Err(StorageError::InvalidRecord(
                "transaction differs from nonce claim",
            ));
        }
        if record.state != StoredEvmTransactionState::Pending {
            return Err(StorageError::InvalidRecord(
                "only pending candidates can be reserved",
            ));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !super::evm_nonces::is_reserved(&transaction, claim)? {
            return Err(StorageError::InvalidRecord(
                "nonce owner retired or already dispatched",
            ));
        }
        let bound = transaction.execute("UPDATE evm_nonce_claims SET signed_hash=?1 WHERE id=?2 AND owner_token=?3 AND phase='reserved' AND signed_hash IS NULL",params![record.transaction_hash,claim.id,claim.owner_token])?;
        if bound != 1 {
            return Err(StorageError::InvalidRecord(
                "signed nonce hash already bound",
            ));
        }
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
            params![record.transaction_hash,claim.owner_token],
        )?;
        transaction.commit()?;
        Ok(())
    }
    /// Atomically fences the exact nonce owner before the one irreversible send.
    /// # Errors
    /// Returns a storage failure. False means retired, already dispatched or unknown.
    pub fn mark_evm_dispatched(
        &mut self,
        hash: &str,
        claim: &EvmNonceClaim,
    ) -> Result<bool, StorageError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = mark_in_transaction(&tx, hash, claim)?;
        tx.commit()?;
        Ok(changed)
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
        &mut self,
        network: &str,
        sender: &str,
    ) -> Result<usize, StorageError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let deleted = tx.execute("DELETE FROM evm_transactions WHERE network=?1 AND lower(sender)=lower(?2) AND state='pending' AND transaction_hash IN (SELECT transaction_hash FROM evm_transfer_dispatches WHERE phase='reserved')",params![network,sender])?;
        tx.execute("UPDATE evm_nonce_claims SET phase='retired' WHERE network=?1 AND sender=lower(?2) AND phase='reserved'",params![network,sender])?;
        tx.commit()?;
        Ok(deleted)
    }
    /// Retires a reservation only when the active worker proves no send occurred.
    /// # Errors
    /// Returns a storage failure; call only when the final admission callback
    /// returned false. A timeout, worker panic or send error is not that proof.
    pub fn discard_unsent_evm_transaction(
        &mut self,
        hash: &str,
        claim: &EvmNonceClaim,
    ) -> Result<(), StorageError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        discard_unsent_in_transaction(&tx, hash, claim)?;
        tx.commit()?;
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

// Shared transaction composition seam: workflow dispatch must be durable in the
// same commit as nonce/send admission. This is not a second nonce policy.
pub(in crate::store) fn mark_in_transaction(
    tx: &rusqlite::Transaction<'_>,
    hash: &str,
    claim: &EvmNonceClaim,
) -> Result<bool, StorageError> {
    if !super::evm_nonces::is_reserved(tx, claim)? {
        return Ok(false);
    }
    let changed = tx.execute("UPDATE evm_nonce_claims SET phase='possible_send' WHERE id=?1 AND owner_token=?2 AND phase='reserved' AND signed_hash=?3 AND EXISTS(SELECT 1 FROM evm_transfer_dispatches d WHERE d.transaction_hash=?3 AND d.reservation_id=?2 AND d.phase='reserved')",params![claim.id,claim.owner_token,hash])? == 1;
    if !changed {
        return Ok(false);
    }
    if tx.execute("UPDATE evm_transfer_dispatches SET phase='dispatched' WHERE transaction_hash=?1 AND reservation_id=?2 AND phase='reserved'",params![hash,claim.owner_token])? != 1 {
        return Err(StorageError::InvalidRecord("nonce dispatch fence changed"));
    }
    Ok(true)
}

// Only the actual controller's callback-false path may use this exact-owner seam.
pub(in crate::store) fn discard_unsent_in_transaction(
    tx: &rusqlite::Transaction<'_>,
    hash: &str,
    claim: &EvmNonceClaim,
) -> Result<bool, StorageError> {
    let deleted = tx.execute("DELETE FROM evm_transactions WHERE transaction_hash=?1 AND state='pending' AND transaction_hash IN (SELECT d.transaction_hash FROM evm_transfer_dispatches d JOIN evm_nonce_claims c ON c.owner_token=d.reservation_id WHERE c.id=?2 AND c.owner_token=?3 AND c.signed_hash=?1 AND c.phase IN ('reserved','possible_send') AND d.phase IN ('reserved','dispatched'))",params![hash,claim.id,claim.owner_token])?;
    if deleted == 1 {
        tx.execute("UPDATE evm_nonce_claims SET phase='retired' WHERE id=?1 AND owner_token=?2 AND signed_hash=?3 AND phase IN ('reserved','possible_send')",params![claim.id,claim.owner_token,hash])?;
    }
    Ok(deleted == 1)
}
