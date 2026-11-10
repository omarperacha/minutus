//! Historical completion and current condition evidence remain distinct.
use super::{
    ContractStepClaim, ContractStepState, ContractWorkflowId, ContractWorkflowStep, generic,
    generic_records::{StoredGenericWorkflowEvidence, StoredWorkflowEvidenceKind},
    records, scope,
};
use crate::{ApplicationStore, StorageError};
use rusqlite::{Connection, TransactionBehavior, params};

// Sixteen ordered writes require at most 1 + 3 * 15 = 46 prior-step
// observations. Retain bounded headroom; this grants no additional attempts.
const MAX_EVIDENCE_RECORDS: usize = 64;

impl ApplicationStore {
    /// Persists final goal observations after core consumes a sealed certificate.
    /// Historical step completion is retained when current goals later change.
    ///
    /// # Errors
    /// Rejects incomplete workflows, changed declaration or storage failure.
    pub fn record_generic_contract_goals(
        &mut self,
        id: &ContractWorkflowId,
        evidence: &StoredGenericWorkflowEvidence,
    ) -> Result<bool, StorageError> {
        if evidence.kind != StoredWorkflowEvidenceKind::Goals {
            return scope::invalid("final goal certificate required");
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let steps = records::load_workflow(&tx, id)?;
        if steps
            .iter()
            .any(|step| step.state != ContractStepState::Verified)
        {
            return scope::invalid("workflow goals require every historical completion");
        }
        let last = steps.last().ok_or(StorageError::CorruptDatabase)?.index;
        validate_evidence(&tx, id, last, evidence)?;
        let inserted = insert_evidence(&tx, id, last, evidence)?;
        tx.commit()?;
        Ok(inserted)
    }
    /// Persists completion after core consumes a sealed chain certificate.
    /// This trusted primitive must never be exposed to model/IPC callers.
    ///
    /// # Errors
    /// Rejects changed declaration, candidate, transaction or execution state.
    pub fn apply_generic_contract_completion(
        &mut self,
        claim: &ContractStepClaim,
        evidence: &StoredGenericWorkflowEvidence,
    ) -> Result<bool, StorageError> {
        if evidence.kind != StoredWorkflowEvidenceKind::Completion {
            return scope::invalid("completion certificate context required");
        }
        let commitment = evidence.commitment()?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let steps = records::load_workflow(&tx, &claim.workflow)?;
        validate_evidence(&tx, &claim.workflow, claim.index, evidence)?;
        let step = steps
            .get(usize::from(claim.index))
            .ok_or(StorageError::CorruptDatabase)?;
        if step.state != ContractStepState::ConfirmedUnverified
            || step.commitment != claim.commitment
        {
            return Ok(false);
        }
        insert_evidence(&tx, &claim.workflow, claim.index, evidence)?;
        let changed=tx.execute("UPDATE contract_workflow_steps SET state='verified',evidence_commitment=?1 WHERE workflow_id=?2 AND step_index=?3 AND commitment=?4 AND state='confirmed_unverified'",params![commitment,claim.workflow.0,claim.index,claim.commitment])?==1;
        records::load_workflow(&tx, &claim.workflow)?;
        tx.commit()?;
        Ok(changed)
    }
    /// Loads ordered evidence without recreating cryptographic authority.
    ///
    /// # Errors
    /// Rejects corrupt contexts or unavailable storage.
    pub fn generic_contract_workflow_evidence(
        &self,
        id: &ContractWorkflowId,
        index: u8,
    ) -> Result<Vec<StoredGenericWorkflowEvidence>, StorageError> {
        records::load_workflow(&self.connection, id)?;
        load_evidence(&self.connection, id, index)
    }
}
pub(super) fn validate_context(
    connection: &Connection,
    id: &ContractWorkflowId,
    steps: &[ContractWorkflowStep],
) -> Result<(), StorageError> {
    let declaration =
        generic::load_declaration(connection, id)?.ok_or(StorageError::CorruptDatabase)?;
    let scope = declaration.commitment()?;
    for step in steps {
        let preparation = generic::load_preparation(connection, id, step.index)?;
        if preparation.as_ref().is_some_and(|preparation| {
            preparation.scope_commitment != scope
                || preparation.intent_commitment != step.commitment
        }) {
            return Err(StorageError::CorruptDatabase);
        }
        if preparation.is_none()
            && matches!(
                step.state,
                ContractStepState::Dispatching
                    | ContractStepState::Submitted
                    | ContractStepState::ConfirmedUnverified
                    | ContractStepState::Verified
                    | ContractStepState::Uncertain
            )
        {
            return Err(StorageError::CorruptDatabase);
        }
        let evidence = load_evidence(connection, id, step.index)?;
        for item in &evidence {
            validate_evidence(connection, id, step.index, item)?;
        }
        if step.state == ContractStepState::Verified
            && !evidence.iter().any(|item| {
                item.kind == StoredWorkflowEvidenceKind::Completion
                    && item.commitment().ok().as_deref() == step.evidence_commitment.as_deref()
            })
        {
            return Err(StorageError::CorruptDatabase);
        }
    }
    Ok(())
}
pub(super) fn validate_evidence(
    connection: &Connection,
    id: &ContractWorkflowId,
    index: u8,
    evidence: &StoredGenericWorkflowEvidence,
) -> Result<(), StorageError> {
    evidence.commitment()?;
    let declaration =
        generic::load_declaration(connection, id)?.ok_or(StorageError::CorruptDatabase)?;
    let step = declaration
        .steps
        .get(usize::from(index))
        .ok_or(StorageError::CorruptDatabase)?;
    let expected = match evidence.kind {
        StoredWorkflowEvidenceKind::Completion => &step.completion,
        StoredWorkflowEvidenceKind::Prerequisites => &step.prerequisites,
        StoredWorkflowEvidenceKind::Goals => &declaration.goals,
    };
    if evidence.kind == StoredWorkflowEvidenceKind::Goals && expected.is_empty() {
        return scope::invalid("undeclared workflow goals cannot be credited");
    }
    if declaration.commitment()? != evidence.scope_commitment
        || evidence
            .results
            .iter()
            .map(|result| &result.condition)
            .ne(expected.iter())
    {
        return scope::invalid("generic evidence differs from declared conditions");
    }
    if evidence.kind != StoredWorkflowEvidenceKind::Goals {
        let preparation = generic::load_preparation(connection, id, index)?
            .ok_or(StorageError::CorruptDatabase)?;
        if evidence.preparation_commitment.as_deref() != Some(preparation.commitment()?.as_str()) {
            return scope::invalid("generic evidence differs from exact preparation");
        }
    }
    if let Some(receipt) = &evidence.receipt {
        let hash:Option<String>=connection.query_row("SELECT transaction_hash FROM contract_workflow_steps WHERE workflow_id=?1 AND step_index=?2",params![id.0,index],|row|row.get(0))?;
        if hash.as_deref() != Some(receipt.transaction_hash.as_str()) {
            return scope::invalid("generic completion transaction differs from stored step");
        }
    }
    Ok(())
}
pub(super) fn insert_evidence(
    connection: &Connection,
    id: &ContractWorkflowId,
    index: u8,
    evidence: &StoredGenericWorkflowEvidence,
) -> Result<bool, StorageError> {
    let commitment = evidence.commitment()?;
    let exists:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM contract_workflow_evidence WHERE workflow_id=?1 AND step_index=?2 AND evidence_commitment=?3)",params![id.0,index,commitment],|row|row.get(0))?;
    if exists {
        return Ok(false);
    }
    let count: u32 = connection.query_row(
        "SELECT count(*) FROM contract_workflow_evidence WHERE workflow_id=?1 AND step_index=?2",
        params![id.0, index],
        |row| row.get(0),
    )?;
    if usize::try_from(count).map_err(|_| StorageError::CorruptDatabase)? >= MAX_EVIDENCE_RECORDS {
        return scope::invalid("generic workflow evidence retention limit reached");
    }
    let json = scope::bounded_json(evidence, scope::MAX_CONTEXT_BYTES)?;
    Ok(connection.execute("INSERT INTO contract_workflow_evidence(workflow_id,step_index,evidence_commitment,evidence_json) VALUES (?1,?2,?3,?4)",params![id.0,index,commitment,json])?==1)
}
fn load_evidence(
    connection: &Connection,
    id: &ContractWorkflowId,
    index: u8,
) -> Result<Vec<StoredGenericWorkflowEvidence>, StorageError> {
    let mut statement=connection.prepare("SELECT evidence_commitment,evidence_json FROM contract_workflow_evidence WHERE workflow_id=?1 AND step_index=?2 ORDER BY id LIMIT 65")?;
    let mut rows = statement.query(params![id.0, index])?;
    let mut result = Vec::new();
    while let Some(row) = rows.next()? {
        let commitment: String = row.get(0)?;
        let json: String = row.get(1)?;
        if json.len() > scope::MAX_CONTEXT_BYTES || result.len() >= MAX_EVIDENCE_RECORDS {
            return Err(StorageError::CorruptDatabase);
        }
        let evidence: StoredGenericWorkflowEvidence =
            serde_json::from_str(&json).map_err(|_| StorageError::CorruptDatabase)?;
        if evidence.commitment()? != commitment
            || scope::bounded_json(&evidence, scope::MAX_CONTEXT_BYTES)? != json
        {
            return Err(StorageError::CorruptDatabase);
        }
        result.push(evidence);
    }
    Ok(result)
}
