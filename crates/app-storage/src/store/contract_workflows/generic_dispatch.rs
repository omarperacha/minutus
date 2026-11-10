//! Generic workflow gates join the existing exact nonce owner in one transaction.
use super::{
    ContractStepClaim, ContractStepState, generic, generic_evidence,
    generic_records::{StoredGenericWorkflowEvidence, StoredWorkflowEvidenceKind},
    records, scope, scoped,
};
use crate::{ApplicationStore, EvmNonceClaim, StorageError};
use rusqlite::{Connection, TransactionBehavior, params};

impl ApplicationStore {
    /// Checks exact preparation liveness before signing; dispatch repeats it.
    ///
    /// # Errors
    /// Rejects corrupt state; false denotes retired native authority.
    pub fn generic_contract_preparation_is_current(
        &self,
        claim: &ContractStepClaim,
        commitment: &str,
    ) -> Result<bool, StorageError> {
        current(&self.connection, claim, commitment)
    }

    /// Atomically persists an independently verified current gate and fences the
    /// exact workflow/nonce owner before transport admission. Raw evidence alone
    /// is not verification; only the application certificate adapter calls this.
    ///
    /// # Errors
    /// Rejects stale scope, transaction, nonce, declaration or preparation.
    pub fn mark_generic_contract_dispatched(
        &mut self,
        claim: &ContractStepClaim,
        commitment: &str,
        signed_hash: &str,
        nonce: &EvmNonceClaim,
        prerequisites: &StoredGenericWorkflowEvidence,
    ) -> Result<bool, StorageError> {
        records::validate_hash(signed_hash)?;
        if prerequisites.kind != StoredWorkflowEvidenceKind::Prerequisites {
            return scope::invalid("current prerequisite certificate required");
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !current(&tx, claim, commitment)? {
            return Ok(false);
        }
        generic_evidence::validate_evidence(&tx, &claim.workflow, claim.index, prerequisites)?;
        validate_nonce(&tx, claim, nonce)?;
        generic_evidence::insert_evidence(&tx, &claim.workflow, claim.index, prerequisites)?;
        if !crate::store::evm_dispatch::mark_in_transaction(&tx, signed_hash, nonce)? {
            return Ok(false);
        }
        let changed=tx.execute("UPDATE contract_workflow_steps SET state='dispatching',transaction_hash=?1 WHERE workflow_id=?2 AND step_index=?3 AND commitment=?4 AND state='reviewing' AND transaction_hash IS NULL",params![signed_hash,claim.workflow.0,claim.index,claim.commitment])?==1;
        if !changed {
            return scope::invalid("generic dispatch fence changed");
        }
        records::load_workflow(&tx, &claim.workflow)?;
        tx.commit()?;
        Ok(true)
    }

    /// Retires only an exact dispatch whose actual admission callback returned
    /// false. Panics, timeouts and missing acknowledgements are not this evidence.
    ///
    /// # Errors
    /// Retains blocking state on changed ownership or any storage failure.
    pub fn discard_known_unsent_generic_contract(
        &mut self,
        claim: &ContractStepClaim,
        hash: &str,
        nonce: &EvmNonceClaim,
    ) -> Result<bool, StorageError> {
        records::validate_hash(hash)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let steps = records::load_workflow(&tx, &claim.workflow)?;
        let step = steps
            .get(usize::from(claim.index))
            .ok_or(StorageError::CorruptDatabase)?;
        if step.state != ContractStepState::Dispatching
            || step.commitment != claim.commitment
            || step.transaction_hash.as_deref() != Some(hash)
        {
            return Ok(false);
        }
        validate_nonce(&tx, claim, nonce)?;
        if !crate::store::evm_dispatch::discard_unsent_in_transaction(&tx, hash, nonce)? {
            return Ok(false);
        }
        let changed=tx.execute("UPDATE contract_workflow_steps SET state='cancelled',transaction_hash=NULL,evidence_commitment=NULL WHERE workflow_id=?1 AND step_index=?2 AND commitment=?3 AND state='dispatching' AND transaction_hash=?4",params![claim.workflow.0,claim.index,claim.commitment,hash])?==1;
        if !changed {
            return scope::invalid("generic known-unsent fence changed");
        }
        records::load_workflow(&tx, &claim.workflow)?;
        tx.commit()?;
        Ok(true)
    }
}
fn current(
    connection: &Connection,
    claim: &ContractStepClaim,
    commitment: &str,
) -> Result<bool, StorageError> {
    records::validate_digest(commitment)?;
    let steps = records::load_workflow(connection, &claim.workflow)?;
    let step = steps
        .get(usize::from(claim.index))
        .ok_or(StorageError::CorruptDatabase)?;
    let preparation = generic::load_preparation(connection, &claim.workflow, claim.index)?
        .ok_or(StorageError::CorruptDatabase)?;
    Ok(step.state == ContractStepState::Reviewing
        && step.commitment == claim.commitment
        && preparation.commitment()? == commitment
        && scoped::current_turn(connection, &claim.workflow)?
        && steps
            .iter()
            .take(usize::from(claim.index))
            .all(|prior| prior.state == ContractStepState::Verified))
}
fn validate_nonce(
    connection: &Connection,
    claim: &ContractStepClaim,
    nonce: &EvmNonceClaim,
) -> Result<(), StorageError> {
    let declaration = generic::load_declaration(connection, &claim.workflow)?
        .ok_or(StorageError::CorruptDatabase)?;
    let preparation = generic::load_preparation(connection, &claim.workflow, claim.index)?
        .ok_or(StorageError::CorruptDatabase)?;
    if nonce.network != declaration.network
        || nonce.sender != declaration.account
        || nonce.nonce != preparation.nonce
        || nonce.candidate_digest != preparation.candidate_digest
    {
        return scope::invalid("generic workflow nonce owner differs from exact candidate");
    }
    Ok(())
}
