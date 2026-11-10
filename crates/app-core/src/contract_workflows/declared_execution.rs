//! Generic workflow ownership, with no deserialisable execution capabilities.
use super::ContractWorkflowDeclaration;
use crate::{AppCore, AppCoreError};
use app_storage::{
    ContractStepClaim, ContractStepState, ContractWorkflowId, ContractWorkflowStep,
    StoredDeclaredWorkflowStep,
};
use ipc_types::{ConversationId, MessageId};
use std::sync::Arc;

/// One current-turn generic declaration. Scope alone grants no write authority.
pub struct DeclaredContractWorkflowSession {
    pub(super) core: Arc<AppCore>,
    pub(super) id: ContractWorkflowId,
    pub(super) declaration: ContractWorkflowDeclaration,
}
impl Drop for DeclaredContractWorkflowSession {
    fn drop(&mut self) {
        // Abandoning source/readiness preparation retires the next unsent
        // attempt. claim_next cannot cross an uncertain/failed/active step.
        // Neither operation touches nonce ownership or transaction history.
        if let Ok(Some(step)) = self.claim_next() {
            let _ = step.stop_before_send(ContractStepState::Failed);
        }
    }
}
/// A consumed ordered attempt, before independent generic candidate binding.
pub struct DeclaredContractWorkflowStep {
    pub(super) core: Arc<AppCore>,
    pub(super) claim: ContractStepClaim,
    pub(super) declaration: ContractWorkflowDeclaration,
    pub(super) intent: StoredDeclaredWorkflowStep,
}
impl Drop for DeclaredContractWorkflowStep {
    fn drop(&mut self) {
        // Journal-only retirement of an abandoned Reviewing attempt. This CAS
        // cannot modify possible sends, completed steps or any nonce ownership.
        let _ = self.stop_before_send(ContractStepState::Failed);
    }
}
impl AppCore {
    /// Atomically starts at most one declared workflow in the persisted user turn.
    ///
    /// # Errors
    /// Rejects stale turns, malformed declarations or storage faults.
    pub fn start_declared_contract_workflow(
        self: &Arc<Self>,
        conversation: &ConversationId,
        message: &MessageId,
        declaration: ContractWorkflowDeclaration,
    ) -> Result<Option<DeclaredContractWorkflowSession>, AppCoreError> {
        let id = self.with_storage(|storage| {
            storage.create_declared_contract_workflow(conversation, message, declaration.intent())
        })?;
        Ok(id.map(|id| DeclaredContractWorkflowSession {
            core: Arc::clone(self),
            id,
            declaration,
        }))
    }
}
impl DeclaredContractWorkflowSession {
    /// Current final goals, separate from historical per-step completion.
    ///
    /// # Errors
    /// Rejects declarations without a qualified observation adapter.
    pub fn goals(
        &self,
    ) -> Result<Vec<chain_evm::contract::generic::GenericCondition>, app_storage::StorageError>
    {
        super::declared_qualification::conditions(
            &self.declaration,
            &self.declaration.intent().goals,
        )
    }

    /// Records fresh final goals only after every exact historical step remains
    /// canonically anchored. Persisted Verified rows alone cannot call this API.
    ///
    /// # Errors
    /// Rejects stale certificates, changed account/scope/history or storage faults.
    pub fn apply_goals(
        &self,
        proof: chain_evm::contract::generic::VerifiedGenericGoals,
    ) -> Result<app_storage::StoredGenericWorkflowEvidence, AppCoreError> {
        let evidence = super::declared_qualification::observation(
            &self.declaration,
            proof.conditions(),
            &self.declaration.intent().goals,
            app_storage::StoredWorkflowEvidenceKind::Goals,
            None,
            None,
        )?;
        self.core.with_storage(|storage| {
            let steps = storage.contract_workflow_steps(&self.id)?;
            for step in &steps {
                let preparation = storage
                    .generic_contract_preparation(&self.id, step.index)?
                    .ok_or(app_storage::StorageError::CorruptDatabase)?;
                if preparation
                    .source_identities
                    .iter()
                    .any(|source| !proof.conditions().source_commitments().contains(source))
                {
                    return Err(app_storage::StorageError::InvalidRecord(
                        "final goals lost a qualified workflow source identity",
                    ));
                }
            }
            if proof.account() != self.declaration.intent().account
                || proof.historical_transactions().len() != steps.len()
                || steps
                    .iter()
                    .zip(proof.historical_transactions())
                    .any(|(step, hash)| {
                        step.state != ContractStepState::Verified
                            || step.transaction_hash.as_deref() != Some(hash.as_str())
                    })
                || !proof.conditions().is_fresh()
            {
                return Err(app_storage::StorageError::InvalidRecord(
                    "final goal certificate differs from complete workflow history",
                ));
            }
            storage.record_generic_contract_goals(&self.id, &evidence)?;
            Ok(())
        })?;
        drop(proof); // Consumed certificate cannot be retained as current authority.
        Ok(evidence)
    }
    /// Immutable exact declaration, without source/effect authority.
    #[must_use]
    pub const fn declaration(&self) -> &ContractWorkflowDeclaration {
        &self.declaration
    }
    /// Host-owned durable identity, never model-supplied authority.
    #[must_use]
    pub const fn id(&self) -> &ContractWorkflowId {
        &self.id
    }
    /// Consumes only the next ordered attempt. This creates no prepared candidate,
    /// native approval or signing capability. Fresh prerequisites are mandatory
    /// when binding a candidate and again after authentication and at dispatch.
    ///
    /// # Errors
    /// Rejects changed declarations, stale turns or storage failure.
    pub fn claim_next(&self) -> Result<Option<DeclaredContractWorkflowStep>, AppCoreError> {
        let claim = self.core.with_storage(|storage| {
            if storage.contract_workflow_declaration(&self.id)?.as_ref()
                != Some(self.declaration.intent())
            {
                return Err(app_storage::StorageError::CorruptDatabase);
            }
            storage.claim_contract_step(&self.id)
        })?;
        Ok(claim.map(|claim| DeclaredContractWorkflowStep {
            core: Arc::clone(&self.core),
            intent: self.declaration.intent().steps[usize::from(claim.index())].clone(),
            claim,
            declaration: self.declaration.clone(),
        }))
    }
    /// Historical execution facts; fresh current requirements remain separate.
    ///
    /// # Errors
    /// Rejects corrupt/unavailable records.
    pub fn steps(&self) -> Result<Vec<ContractWorkflowStep>, AppCoreError> {
        self.core
            .with_storage(|storage| storage.contract_workflow_steps(&self.id))
    }
}
impl DeclaredContractWorkflowStep {
    /// Immutable workflow declaration used by source/condition preflight.
    #[must_use]
    pub const fn declaration(&self) -> &ContractWorkflowDeclaration {
        &self.declaration
    }

    /// Exact fresh current requirements, without minting an observation.
    ///
    /// # Errors
    /// Rejects unsupported predicate engines or invalid aliases.
    pub fn prerequisites(
        &self,
    ) -> Result<Vec<chain_evm::contract::generic::GenericCondition>, app_storage::StorageError>
    {
        super::declared_qualification::conditions(&self.declaration, &self.intent.prerequisites)
    }
    /// Declared completion conditions for trusted pre-review prediction.
    ///
    /// # Errors
    /// Rejects unsupported condition engines and invalid contract references.
    pub fn completion_conditions(
        &self,
    ) -> Result<Vec<chain_evm::contract::generic::GenericCondition>, app_storage::StorageError>
    {
        super::declared_qualification::conditions(&self.declaration, &self.intent.completion)
    }

    /// Exact immutable generic call declaration.
    #[must_use]
    pub const fn intent(&self) -> &StoredDeclaredWorkflowStep {
        &self.intent
    }
    /// Ordered host-selected step index.
    #[must_use]
    pub const fn index(&self) -> u8 {
        self.claim.index()
    }
    /// Stops an unsubmitted attempt without creating another write allowance.
    ///
    /// # Errors
    /// Rejects illegal or stale lifecycle transitions.
    pub fn stop_before_send(&self, state: ContractStepState) -> Result<bool, AppCoreError> {
        if !matches!(
            state,
            ContractStepState::Denied | ContractStepState::Cancelled | ContractStepState::Failed
        ) {
            return Err(app_storage::StorageError::InvalidRecord(
                "invalid declared workflow terminal state",
            )
            .into());
        }
        self.core.with_storage(|storage| {
            storage.record_contract_step(&self.claim, ContractStepState::Reviewing, state, None)
        })
    }
}
