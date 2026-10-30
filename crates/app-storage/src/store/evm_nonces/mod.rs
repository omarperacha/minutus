//! Shared EOA nonce ownership; never approval, keys or permission to replace.
mod claim;
#[cfg(test)]
mod tests;

use super::{ApplicationStore, StorageError, TransactionBehavior, params};
pub use claim::EvmNonceClaim;
pub(super) use claim::is_reserved;

impl ApplicationStore {
    /// Claims exact chain/account/nonce and candidate before authentication.
    /// Owner identities and database fences cannot be reused after retirement.
    ///
    /// # Errors
    /// Rejects invalid bindings or storage faults. `None` means already owned;
    /// acknowledgement, receipt failure and missing hashes do not free a nonce.
    pub fn claim_evm_nonce(
        &mut self,
        network: &str,
        sender: &str,
        nonce: &str,
        candidate_digest: &str,
        owner_token: &str,
    ) -> Result<Option<EvmNonceClaim>, StorageError> {
        let mut claim =
            EvmNonceClaim::validated(network, sender, nonce, candidate_digest, owner_token)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let inserted = tx.execute("INSERT INTO evm_nonce_claims(network,sender,nonce,candidate_digest,owner_token,phase) VALUES (?1,?2,?3,?4,?5,'reserved') ON CONFLICT DO NOTHING",params![claim.network,claim.sender,claim.nonce,claim.candidate_digest,claim.owner_token])?;
        if inserted != 1 {
            return Ok(None);
        }
        claim.id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(Some(claim))
    }

    /// Checks this exact owner's liveness before signing, without granting authority.
    ///
    /// # Errors
    /// Returns storage failure; false blocks stale signing and dispatch.
    pub fn evm_nonce_is_current(&self, claim: &EvmNonceClaim) -> Result<bool, StorageError> {
        claim::is_reserved(&self.connection, claim)
    }

    /// Releases only the exact owner while no dispatch admission was possible.
    /// Safe on denial/signing/revalidation failure; never frees `possible_send`.
    ///
    /// # Errors
    /// Storage failure leaves the nonce conservatively blocked.
    pub fn release_unsent_evm_nonce(
        &mut self,
        claim: &EvmNonceClaim,
    ) -> Result<bool, StorageError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !claim::is_reserved(&tx, claim)? {
            return Ok(false);
        }
        claim::retire_unsent(&tx, claim)?;
        tx.commit()?;
        Ok(true)
    }
}
