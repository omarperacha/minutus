//! Pre-unlock turn admission is durable; later scope attachment cannot retry it.
use super::{
    ContractWorkflowDeclaration, ContractWorkflowScope, ContractWorkflowSession,
    DeclaredContractWorkflowSession,
};
use crate::{AppCore, AppCoreError};
use app_storage::ContractWorkflowAttemptClaim;
use ipc_types::{ConversationId, MessageId};
use std::sync::Arc;

/// The one current persisted turn attempt, bound to its live application owner.
/// No selected account is needed to reserve it before native unlock. It is not
/// cloneable/serialisable/reconstructible, and grants no approval or signing.
/// Cancellation, error or drop leaves its durable tombstone consumed.
#[must_use = "dropping this attempt does not allow another attempt in the turn"]
pub struct ContractWorkflowAttempt {
    core: Arc<AppCore>,
    claim: ContractWorkflowAttemptClaim,
}
impl AppCore {
    /// Durably reserves the shared transfer/v1/v2 allowance before native unlock.
    /// Caller cancellation must drop the returned attempt, never reclaim by ID.
    ///
    /// # Errors
    /// Rejects storage failure; None means an invalid, stale or already-used turn.
    pub fn claim_contract_workflow_attempt(
        self: &Arc<Self>,
        conversation: &ConversationId,
        message: &MessageId,
    ) -> Result<Option<ContractWorkflowAttempt>, AppCoreError> {
        let claim = self.with_storage(|storage| {
            storage.claim_contract_workflow_attempt(conversation, message)
        })?;
        Ok(claim.map(|claim| ContractWorkflowAttempt {
            core: Arc::clone(self),
            claim,
        }))
    }
}
impl ContractWorkflowAttempt {
    /// After unlock, consumes this attempt into the exact selected-account v1
    /// scope. Storage rechecks current persisted user/turn and existing workflow.
    ///
    /// # Errors
    /// Rejects storage faults. Stale attachment returns None without a refund.
    pub fn start_profile(
        self,
        scope: ContractWorkflowScope,
    ) -> Result<Option<ContractWorkflowSession>, AppCoreError> {
        let Self { core, claim } = self;
        let id = core.with_storage(|storage| {
            storage.attach_scoped_contract_workflow(claim, scope.intent())
        })?;
        Ok(id.map(|id| ContractWorkflowSession { core, id, scope }))
    }
    /// After unlock, consumes this attempt into the exact selected-account v2
    /// declaration. It never mints evidence, approval, authentication or nonce.
    ///
    /// # Errors
    /// Rejects storage faults. Stale attachment returns None without a refund.
    pub fn start_declared(
        self,
        declaration: ContractWorkflowDeclaration,
    ) -> Result<Option<DeclaredContractWorkflowSession>, AppCoreError> {
        let Self { core, claim } = self;
        let id = core.with_storage(|storage| {
            storage.attach_declared_contract_workflow(claim, declaration.intent())
        })?;
        Ok(id.map(|id| DeclaredContractWorkflowSession {
            core,
            id,
            declaration,
        }))
    }
}

#[cfg(test)]
mod tests;
