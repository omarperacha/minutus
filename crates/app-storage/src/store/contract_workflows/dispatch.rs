//! Workflow dispatch is composed with the shared nonce fence in one transaction.
use super::{ContractStepClaim, ContractStepState, records, scope, scoped};
use crate::{ApplicationStore, EvmNonceClaim, StorageError};
use rusqlite::{TransactionBehavior, params};

impl ApplicationStore {
    /// Checks prepared authority before the signer releases a signature. The
    /// dispatch transaction repeats these checks and remains the final fence.
    ///
    /// # Errors
    /// Rejects malformed or unavailable state; false means retired authority.
    pub fn contract_step_preparation_is_current(
        &self,
        step: &ContractStepClaim,
        commitment: &str,
    ) -> Result<bool, StorageError> {
        let steps = records::load_workflow(&self.connection, &step.workflow)?;
        let Some(current) = steps.get(usize::from(step.index)) else {
            return Err(StorageError::CorruptDatabase);
        };
        let candidate = scoped::load_candidate(&self.connection, &step.workflow, step.index)?
            .ok_or(StorageError::CorruptDatabase)?;
        Ok(current.state == ContractStepState::Reviewing
            && current.commitment == step.commitment
            && candidate.commitment()? == commitment
            && scoped::current_turn(&self.connection, &step.workflow)?
            && steps
                .iter()
                .take(usize::from(step.index))
                .all(|prior| prior.state == ContractStepState::Verified))
    }

    /// Retires an exact workflow/nonce dispatch only after the actual final
    /// admission callback returned false. Never call after a true callback,
    /// transport timeout, panic or unknown result. Raw signed-hash tombstones stay.
    ///
    /// # Errors
    /// Rejects a changed state/binding, contradictory observation or storage fault.
    pub fn discard_known_unsent_contract_workflow(
        &mut self,
        step: &ContractStepClaim,
        signed_hash: &str,
        nonce: &EvmNonceClaim,
    ) -> Result<bool, StorageError> {
        records::validate_hash(signed_hash)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let steps = records::load_workflow(&tx, &step.workflow)?;
        let current = steps
            .get(usize::from(step.index))
            .ok_or(StorageError::CorruptDatabase)?;
        if current.state != ContractStepState::Dispatching
            || current.commitment != step.commitment
            || current.transaction_hash.as_deref() != Some(signed_hash)
        {
            return Ok(false);
        }
        let observed: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM contract_workflow_observations WHERE workflow_id=?1 AND step_index=?2)",params![step.workflow.0,step.index],|row|row.get(0))?;
        if observed {
            return scope::invalid("observed dispatch cannot be declared unsent");
        }
        let candidate = scoped::load_candidate(&tx, &step.workflow, step.index)?
            .ok_or(StorageError::CorruptDatabase)?;
        let scope =
            scoped::load_scope(&tx, &step.workflow)?.ok_or(StorageError::CorruptDatabase)?;
        if nonce.network != scope.network
            || nonce.sender != scope.account
            || nonce.nonce != candidate.nonce
            || nonce.candidate_digest != candidate.candidate_digest
        {
            return scope::invalid("wrong workflow nonce owner");
        }
        if !crate::store::evm_dispatch::discard_unsent_in_transaction(&tx, signed_hash, nonce)? {
            return Ok(false);
        }
        let changed = tx.execute("UPDATE contract_workflow_steps SET state='cancelled',transaction_hash=NULL,evidence_commitment=NULL WHERE workflow_id=?1 AND step_index=?2 AND commitment=?3 AND state='dispatching' AND transaction_hash=?4",params![step.workflow.0,step.index,step.commitment,signed_hash])? == 1;
        if !changed {
            return scope::invalid("known-unsent workflow state changed");
        }
        records::load_workflow(&tx, &step.workflow)?;
        tx.commit()?;
        Ok(true)
    }

    /// Atomically commits workflow dispatch and the shared exact nonce/send fence.
    /// The signed hash must already have been reserved by the nonce owner. The
    /// controller may return true to transport only after this method succeeds.
    ///
    /// # Errors
    /// Rejects missing/stale bindings, prerequisites, turn or nonce ownership.
    pub fn mark_contract_workflow_dispatched(
        &mut self,
        step: &ContractStepClaim,
        candidate_commitment: &str,
        signed_hash: &str,
        nonce: &EvmNonceClaim,
    ) -> Result<bool, StorageError> {
        records::validate_digest(candidate_commitment)?;
        records::validate_hash(signed_hash)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let steps = records::load_workflow(&tx, &step.workflow)?;
        let scope = scoped::load_scope(&tx, &step.workflow)?
            .ok_or(StorageError::InvalidRecord("scoped workflow required"))?;
        let candidate = scoped::load_candidate(&tx, &step.workflow, step.index)?.ok_or(
            StorageError::InvalidRecord("prepared workflow candidate required"),
        )?;
        let current = steps
            .get(usize::from(step.index))
            .ok_or(StorageError::CorruptDatabase)?;
        if current.state != ContractStepState::Reviewing
            || current.commitment != step.commitment
            || !scoped::current_turn(&tx, &step.workflow)?
            || steps
                .iter()
                .take(usize::from(step.index))
                .any(|prior| prior.state != ContractStepState::Verified)
        {
            return Ok(false);
        }
        if candidate.commitment()? != candidate_commitment
            || candidate.intent_commitment != step.commitment
            || nonce.network != scope.network
            || nonce.sender != scope.account
            || nonce.nonce != candidate.nonce
            || nonce.candidate_digest != candidate.candidate_digest
        {
            return scope::invalid("nonce/candidate differs from fixed workflow binding");
        }
        if !crate::store::evm_dispatch::mark_in_transaction(&tx, signed_hash, nonce)? {
            return Ok(false);
        }
        let changed = tx.execute("UPDATE contract_workflow_steps SET state='dispatching',transaction_hash=?1 WHERE workflow_id=?2 AND step_index=?3 AND commitment=?4 AND state='reviewing' AND transaction_hash IS NULL", params![signed_hash, step.workflow.0, step.index, step.commitment])? == 1;
        if !changed {
            return scope::invalid("workflow dispatch fence changed");
        }
        records::load_workflow(&tx, &step.workflow)?;
        tx.commit()?;
        Ok(true)
    }
}
