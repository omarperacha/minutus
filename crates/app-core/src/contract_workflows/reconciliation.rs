//! Read-only reconciliation consumes inclusion evidence without restoring a write.
use crate::{AppCore, AppCoreError};
use app_storage::{ContractStepState, StoredContractReconciliation, StoredWorkflowDeclaration};
use chain_evm::contract::VerifiedSubmittedTransaction;
use ipc_types::ConversationId;

/// Existing dispatched transaction context. It contains no claim or signer.
pub struct ContractReconciliation {
    stored: StoredContractReconciliation,
}
impl ContractReconciliation {
    /// Immutable scope for outbound consent and exact transaction verification.
    #[must_use]
    pub const fn declaration(&self) -> &StoredWorkflowDeclaration {
        self.stored.declaration()
    }
    /// Exact host-retained hash; callers cannot replace it.
    #[must_use]
    pub fn transaction_hash(&self) -> &str {
        self.stored.transaction_hash()
    }
    /// Original canonical account nonce.
    #[must_use]
    pub fn nonce(&self) -> &str {
        &self.stored.preparation().nonce
    }
    /// Exact original unsigned EIP-1559 body digest, not candidate/effect labels.
    #[must_use]
    pub fn signing_digest(&self) -> &str {
        &self.stored.preparation().signing_digest
    }
    /// Original step position; this does not grant access to a future step.
    #[must_use]
    pub const fn step_index(&self) -> u8 {
        self.stored.index()
    }
}
impl AppCore {
    /// Looks up a known possible submission only within the current conversation.
    /// # Errors
    /// Rejects corrupt storage; absence must not fall back to an arbitrary hash.
    pub fn contract_reconciliation(
        &self,
        conversation: &ConversationId,
        hash: &str,
    ) -> Result<Option<ContractReconciliation>, AppCoreError> {
        self.with_storage(|store| store.contract_reconciliation(conversation, hash))
            .map(|stored| stored.map(|stored| ContractReconciliation { stored }))
    }
    /// Persists only exact receipt inclusion, retiring all remaining unsent work.
    /// A successful receipt never creates a generic completion certificate.
    /// # Errors
    /// Rejects changed hash/account/nonce/network/body or storage binding.
    pub fn apply_contract_reconciliation(
        &self,
        context: &ContractReconciliation,
        proof: VerifiedSubmittedTransaction,
        observed_at: &str,
    ) -> Result<ContractStepState, AppCoreError> {
        if proof.transaction_hash() != context.transaction_hash()
            || proof.account() != context.declaration().account
            || proof.network().descriptor().caip2_id() != context.declaration().network
            || proof.nonce() != context.nonce()
            || proof.signing_digest() != context.signing_digest()
        {
            return Err(app_storage::StorageError::InvalidRecord(
                "receipt proof differs from stored signed transaction",
            )
            .into());
        }
        let result = self.with_storage(|store| {
            store.record_contract_reconciliation(
                &context.stored,
                proof.succeeded(),
                &proof.block_number().to_string(),
                proof.block_hash(),
                observed_at,
            )
        });
        drop(proof);
        result
    }
}
