use super::ContractWorkflowScope;
use crate::{AppCore, AppCoreError, EvmNonceReservation};
use app_storage::{
    ContractStepClaim, ContractStepState, ContractWorkflowId, ContractWorkflowStep,
    StoredWorkflowCandidate, StoredWorkflowIntent,
};
use ipc_types::{ConversationId, MessageId};
use std::sync::Arc;

/// Host-owned workflow session. Identifiers/structural scope do not approve writes.
pub struct ContractWorkflowSession {
    core: Arc<AppCore>,
    id: ContractWorkflowId,
    scope: ContractWorkflowScope,
}
/// One consumed ordered intent. No approval, authentication or qualified result.
pub struct ContractWorkflowStepWork {
    pub(super) core: Arc<AppCore>,
    pub(super) claim: ContractStepClaim,
    pub(super) intent: StoredWorkflowIntent,
    pub(super) scope: ContractWorkflowScope,
}
impl AppCore {
    /// Atomically starts at most one fixed-scope workflow for the host's user turn.
    /// Model call IDs, aliases and plan revisions do not mint another attempt.
    ///
    /// # Errors
    /// Rejects stale turns, corrupt scope/records or unavailable storage.
    pub fn start_contract_workflow(
        self: &Arc<Self>,
        conversation: &ConversationId,
        message: &MessageId,
        scope: ContractWorkflowScope,
    ) -> Result<Option<ContractWorkflowSession>, AppCoreError> {
        let id = self.with_storage(|storage| {
            storage.create_scoped_contract_workflow(conversation, message, scope.intent())
        })?;
        Ok(id.map(|id| ContractWorkflowSession {
            core: Arc::clone(self),
            id,
            scope,
        }))
    }
}
impl ContractWorkflowSession {
    /// Immutable semantic scope, not an execution/approval capability.
    #[must_use]
    pub const fn scope(&self) -> &ContractWorkflowScope {
        &self.scope
    }
    /// Opaque host-owned workflow identity; no signing authority is carried.
    #[must_use]
    pub const fn id(&self) -> &ContractWorkflowId {
        &self.id
    }
    /// Claims exactly the next step after the host has refreshed prerequisites.
    /// Persisted Verified status is not itself a fresh chain observation.
    ///
    /// # Errors
    /// Rejects inconsistent scope, stale turns, stopped workflows or storage faults.
    pub fn claim_next(
        &self,
        proofs: Vec<chain_evm::contract::VerifiedReceiptPredicates>,
    ) -> Result<Option<ContractWorkflowStepWork>, AppCoreError> {
        let steps = self.steps()?;
        let Some(next) = steps
            .iter()
            .find(|step| step.state != ContractStepState::Verified)
        else {
            return Ok(None);
        };
        if next.state != ContractStepState::Planned {
            return Ok(None);
        }
        super::qualified::apply_prerequisites(
            &self.core,
            &self.id,
            &self.scope,
            next.index,
            proofs,
        )?;
        let claim = self.core.with_storage(|storage| {
            if storage.contract_workflow_scope(&self.id)?.as_ref() != Some(self.scope.intent()) {
                return Err(app_storage::StorageError::CorruptDatabase);
            }
            storage.claim_contract_step(&self.id)
        })?;
        Ok(claim.map(|claim| ContractWorkflowStepWork {
            intent: self.scope.intent().steps[usize::from(claim.index())].clone(),
            core: Arc::clone(&self.core),
            scope: self.scope.clone(),
            claim,
        }))
    }
    /// Historical public lifecycle facts; receipt and task success remain distinct.
    ///
    /// # Errors
    /// Rejects corrupt or unavailable persistent records.
    pub fn steps(&self) -> Result<Vec<ContractWorkflowStep>, AppCoreError> {
        self.core
            .with_storage(|storage| storage.contract_workflow_steps(&self.id))
    }
}
impl ContractWorkflowStepWork {
    /// Atomically retires a workflow and nonce only when the actual transport
    /// admission callback returned false. Errors/timeout/panic are not that proof.
    ///
    /// # Errors
    /// Rejects changed state/owner or conservatively retains on storage failure.
    pub(super) fn discard_known_unsent(
        &self,
        hash: &str,
        nonce: &EvmNonceReservation,
    ) -> Result<bool, AppCoreError> {
        self.core.with_storage(|storage| {
            storage.discard_known_unsent_contract_workflow(&self.claim, hash, nonce.claim())
        })
    }

    /// Fixed semantic template for independent qualification/preparation.
    #[must_use]
    pub const fn intent(&self) -> &StoredWorkflowIntent {
        &self.intent
    }
    /// Semantic intent digest, excluding fresh candidate state/fees.
    #[must_use]
    pub fn intent_commitment(&self) -> &str {
        self.claim.intent_commitment()
    }
    /// Ordered step position, selected by the host journal.
    #[must_use]
    pub const fn index(&self) -> u8 {
        self.claim.index()
    }
    /// Persists structural candidate context only. This does not qualify it and
    /// is not exposed as an approved candidate/native signing route.
    ///
    /// # Errors
    /// Rejects mismatched/rebound context, stopped state or storage failure.
    pub(super) fn bind_candidate_context(
        &self,
        candidate: &StoredWorkflowCandidate,
    ) -> Result<bool, AppCoreError> {
        self.core
            .with_storage(|storage| storage.bind_contract_step_candidate(&self.claim, candidate))
    }
    /// Records denial/cancellation/preparation failure before a send was admitted.
    ///
    /// # Errors
    /// Rejects nonterminal/stale transitions or storage faults.
    pub fn stop_before_send(&self, state: ContractStepState) -> Result<bool, AppCoreError> {
        if !matches!(
            state,
            ContractStepState::Denied | ContractStepState::Cancelled | ContractStepState::Failed
        ) {
            return Err(app_storage::StorageError::InvalidRecord(
                "invalid pre-send workflow terminal state",
            )
            .into());
        }
        self.core.with_storage(|storage| {
            storage.record_contract_step(&self.claim, ContractStepState::Reviewing, state, None)
        })
    }
    /// Atomically fences current scope/prerequisites with the shared nonce owner.
    /// Only the trusted native controller may invoke this after exact approval,
    /// fresh authentication and revalidation. This method does not perform a send.
    ///
    /// # Errors
    /// Rejects changed context, current turn/prerequisites or exact nonce ownership.
    pub(super) fn mark_dispatched(
        &self,
        candidate_commitment: &str,
        hash: &str,
        nonce: &EvmNonceReservation,
    ) -> Result<bool, AppCoreError> {
        self.core.with_storage(|storage| {
            storage.mark_contract_workflow_dispatched(
                &self.claim,
                candidate_commitment,
                hash,
                nonce.claim(),
            )
        })
    }
}
