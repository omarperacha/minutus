//! Owns nonce fencing lifetime across native approval, signing and dispatch.
use crate::{AppCore, AppCoreError, EvmNonceClaim, StoredEvmTransaction};
use std::sync::Arc;

/// Host-only nonce lifetime guard. No signing or approval authority is carried.
/// Drop releases only a definitely unsent reservation; possible sends survive.
pub struct EvmNonceReservation {
    core: Arc<AppCore>,
    claim: EvmNonceClaim,
}
impl EvmNonceReservation {
    pub(crate) fn claim(&self) -> &EvmNonceClaim {
        &self.claim
    }

    /// Verifies that this exact owner is still live before signing.
    ///
    /// # Errors
    /// Returns storage failure; false means the owner was retired or dispatched.
    pub fn is_current(&self) -> Result<bool, AppCoreError> {
        self.core
            .with_storage(|storage| storage.evm_nonce_is_current(&self.claim))
    }

    /// Binds one exact signed hash and its monitoring record to this owner.
    ///
    /// # Errors
    /// Rejects changed facts, stale owners, repeated binding or storage faults.
    pub fn reserve_transaction(&self, record: &StoredEvmTransaction) -> Result<(), AppCoreError> {
        self.core.reserve_evm_transaction(record, &self.claim)
    }
    /// Fences this owner exactly once immediately before transport admission.
    ///
    /// # Errors
    /// Returns storage failure; false must prevent signing-byte submission.
    pub fn mark_dispatched(&self, hash: &str) -> Result<bool, AppCoreError> {
        self.core.mark_evm_dispatched(hash, &self.claim)
    }
    /// Retires a dispatched reservation only after the controller proves final
    /// admission callback returned false. Never call on timeout, panic or a
    /// callback that returned true, even if transport subsequently failed.
    ///
    /// # Errors
    /// A failed cleanup leaves conservative durable blocking metadata.
    pub fn discard_known_unsent(&self, hash: &str) -> Result<(), AppCoreError> {
        self.core.discard_unsent_evm_transaction(hash, &self.claim)
    }
}
impl Drop for EvmNonceReservation {
    fn drop(&mut self) {
        let _ = self
            .core
            .with_storage(|storage| storage.release_unsent_evm_nonce(&self.claim));
    }
}

impl AppCore {
    /// Claims the exact EOA nonce after native approval and before fresh password
    /// authentication. Wallet, chat and future workflows share this database key.
    /// x402 random authorisation nonces remain in their separate protocol ledger.
    ///
    /// # Errors
    /// Storage faults fail closed. `None` means owned, consumed or an owner ID
    /// was reused; neither acknowledgement nor restart clears a possible send.
    pub fn claim_evm_nonce(
        self: &Arc<Self>,
        network: &str,
        sender: &str,
        nonce: &str,
        digest: &str,
        owner_token: &str,
    ) -> Result<Option<EvmNonceReservation>, AppCoreError> {
        let claim = self.with_storage(|storage| {
            storage.claim_evm_nonce(network, sender, nonce, digest, owner_token)
        })?;
        Ok(claim.map(|claim| EvmNonceReservation {
            core: Arc::clone(self),
            claim,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guards_release_only_definite_unsent_and_survive_history_and_acknowledgement()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let core = Arc::new(AppCore::initialize(temp.path())?);
        let record = StoredEvmTransaction {
            transaction_hash: format!("0x{}", "11".repeat(32)),
            network: "eip155:8453".into(),
            sender: "0x1111111111111111111111111111111111111111".into(),
            nonce: "7".into(),
            candidate_digest: format!("0x{}", "22".repeat(32)),
            state: app_storage::StoredEvmTransactionState::Pending,
            block_number: None,
            block_hash: None,
            submitted_at: "2026-10-30T00:00:00Z".into(),
            updated_at: "2026-10-30T00:00:00Z".into(),
        };
        let acquire = |owner| {
            core.claim_evm_nonce(
                &record.network,
                &record.sender,
                &record.nonce,
                &record.candidate_digest,
                owner,
            )
        };
        let first = acquire("first")?.ok_or("claim")?;
        assert!(first.is_current()?);
        assert!(acquire("competing-chat")?.is_none());
        drop(first);
        let second = acquire("second")?.ok_or("claim")?;
        second.reserve_transaction(&record)?;
        core.retire_evm_reservations(&record.network, &record.sender)?;
        assert!(!second.is_current()?);
        assert!(!second.mark_dispatched(&record.transaction_hash)?);
        let third = acquire("third")?.ok_or("claim")?;
        third.reserve_transaction(&record)?;
        assert!(third.mark_dispatched(&record.transaction_hash)?);
        drop(second);
        drop(third);
        assert_eq!(core.evm_transactions()?.len(), 1);
        assert!(core.acknowledge_evm_uncertainty(&record.transaction_hash)?);
        assert!(acquire("fourth")?.is_none());
        Ok(())
    }
}
