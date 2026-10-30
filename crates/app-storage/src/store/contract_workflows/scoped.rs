use super::{
    ContractStepClaim, ContractStepState, ContractWorkflowId, ContractWorkflowStep,
    StoredWorkflowScope, binding::StoredWorkflowCandidate, records, scope,
};
use crate::{ApplicationStore, StorageError};
use ipc_types::{ConversationId, MessageId};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

impl ApplicationStore {
    /// Atomically records fixed semantic scope and consumes the current turn once.
    /// This is structural intent, never native approval or source qualification.
    ///
    /// # Errors
    /// Rejects malformed/oversized scope, stale turns, corruption or storage faults.
    pub fn create_scoped_contract_workflow(
        &mut self,
        conversation: &ConversationId,
        message: &MessageId,
        scope: &StoredWorkflowScope,
    ) -> Result<Option<ContractWorkflowId>, StorageError> {
        let commitment = scope.commitment()?;
        let steps = scope.step_commitments()?;
        let json = scope::bounded_json(scope, scope::MAX_SCOPE_BYTES)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(id) = create_manifest(&tx, conversation, message, &commitment, &steps, true)?
        else {
            return Ok(None);
        };
        tx.execute("INSERT INTO contract_workflow_scopes(workflow_id,format_version,scope_json) VALUES (?1,1,?2)",params![id.0,json])?;
        tx.commit()?;
        Ok(Some(id))
    }
    /// Loads and independently checks the exact semantic scope against its manifest.
    /// Legacy digest-only journals return None and cannot enter scoped execution.
    ///
    /// # Errors
    /// Rejects corrupt, noncanonical or unsupported persisted context.
    pub fn contract_workflow_scope(
        &self,
        id: &ContractWorkflowId,
    ) -> Result<Option<StoredWorkflowScope>, StorageError> {
        load_scope(&self.connection, id)
    }

    /// Binds one fresh exact candidate to a previously consumed semantic step.
    /// A later refresh cannot replace this binding; no approval is persisted.
    ///
    /// # Errors
    /// Rejects mismatch, absent scope, corrupt context, or storage failure.
    pub fn bind_contract_step_candidate(
        &mut self,
        claim: &ContractStepClaim,
        candidate: &StoredWorkflowCandidate,
    ) -> Result<bool, StorageError> {
        let commitment = candidate.commitment()?;
        if candidate.intent_commitment != claim.commitment {
            return scope::invalid("candidate differs from semantic intent");
        }
        let json = scope::bounded_json(candidate, scope::MAX_CONTEXT_BYTES)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let steps = records::load_workflow(&tx, &claim.workflow)?;
        let scope = load_scope(&tx, &claim.workflow)?.ok_or(StorageError::InvalidRecord(
            "scoped workflow context required",
        ))?;
        let step = steps
            .get(usize::from(claim.index))
            .ok_or(StorageError::CorruptDatabase)?;
        if step.state != ContractStepState::Reviewing
            || step.commitment != claim.commitment
            || !current_turn(&tx, &claim.workflow)?
        {
            return Ok(false);
        }
        // Scope/profile equality is checked again by the application's opaque
        // qualified candidate adapter. Persistence only validates its context.
        scope.validate()?;
        let inserted = tx.execute("INSERT INTO contract_workflow_candidates(workflow_id,step_index,candidate_commitment,context_json) VALUES (?1,?2,?3,?4) ON CONFLICT DO NOTHING",params![claim.workflow.0,claim.index,commitment,json])? == 1;
        tx.commit()?;
        Ok(inserted)
    }
    /// Loads exact candidate/observer data, without recreating any authority.
    ///
    /// # Errors
    /// Rejects changed bindings, malformed context or storage failure.
    pub fn contract_step_candidate(
        &self,
        id: &ContractWorkflowId,
        index: u8,
    ) -> Result<Option<StoredWorkflowCandidate>, StorageError> {
        records::load_workflow(&self.connection, id)?;
        load_candidate(&self.connection, id, index)
    }
}

pub(super) fn create_manifest(
    tx: &Transaction<'_>,
    conversation: &ConversationId,
    message: &MessageId,
    scope: &str,
    steps: &[String],
    scoped: bool,
) -> Result<Option<ContractWorkflowId>, StorageError> {
    let claimed = tx.execute("INSERT INTO chat_transfer_attempts (conversation_id,message_id) SELECT ?1,?2 WHERE EXISTS(SELECT 1 FROM messages m WHERE m.id=?2 AND m.conversation_id=?1 AND m.role='user' AND NOT EXISTS(SELECT 1 FROM messages newer WHERE newer.conversation_id=?1 AND newer.role='user' AND newer.position>m.position)) ON CONFLICT DO NOTHING",params![conversation.0,message.0])?;
    if claimed != 1 {
        return Ok(None);
    }
    tx.execute("INSERT INTO contract_workflows(conversation_id,message_id,scope_commitment,step_commitments_json,scope_format) VALUES (?1,?2,?3,?4,?5)",params![conversation.0,message.0,scope,serde_json::to_string(steps).map_err(|_|StorageError::InvalidRecord("invalid workflow manifest"))?,i64::from(scoped)])?;
    let id = ContractWorkflowId(tx.last_insert_rowid());
    for (index, commitment) in steps.iter().enumerate() {
        tx.execute("INSERT INTO contract_workflow_steps(workflow_id,step_index,commitment,state) VALUES (?1,?2,?3,'planned')",params![id.0,i64::try_from(index).map_err(|_|StorageError::InvalidRecord("invalid step index"))?,commitment])?;
    }
    Ok(Some(id))
}
pub(super) fn current_turn(
    connection: &Connection,
    id: &ContractWorkflowId,
) -> Result<bool, StorageError> {
    Ok(connection.query_row("SELECT EXISTS(SELECT 1 FROM contract_workflows w JOIN messages m ON m.id=w.message_id AND m.conversation_id=w.conversation_id AND m.role='user' WHERE w.id=?1 AND NOT EXISTS(SELECT 1 FROM messages newer WHERE newer.conversation_id=w.conversation_id AND newer.role='user' AND newer.position>m.position))",params![id.0],|row|row.get(0))?)
}
pub(super) fn load_scope(
    connection: &Connection,
    id: &ContractWorkflowId,
) -> Result<Option<StoredWorkflowScope>, StorageError> {
    let row: Option<(i64, String, String, String)> = connection.query_row("SELECT s.format_version,s.scope_json,w.scope_commitment,w.step_commitments_json FROM contract_workflow_scopes s JOIN contract_workflows w ON w.id=s.workflow_id WHERE s.workflow_id=?1 AND w.scope_format=1 AND length(s.scope_json)<=65536 AND length(w.step_commitments_json)<=2048",params![id.0],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional()?;
    let Some((version, json, commitment, manifest)) = row else {
        let exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM contract_workflow_scopes WHERE workflow_id=?1) OR EXISTS(SELECT 1 FROM contract_workflows WHERE id=?1 AND scope_format=1)",
            params![id.0],
            |row| row.get(0),
        )?;
        return if exists {
            Err(StorageError::CorruptDatabase)
        } else {
            Ok(None)
        };
    };
    let scope: StoredWorkflowScope =
        serde_json::from_str(&json).map_err(|_| StorageError::CorruptDatabase)?;
    let manifest: Vec<String> =
        serde_json::from_str(&manifest).map_err(|_| StorageError::CorruptDatabase)?;
    if version != 1
        || scope.commitment()? != commitment
        || scope.step_commitments()? != manifest
        || scope::bounded_json(&scope, scope::MAX_SCOPE_BYTES)? != json
    {
        return Err(StorageError::CorruptDatabase);
    }
    Ok(Some(scope))
}
pub(super) fn load_candidate(
    connection: &Connection,
    id: &ContractWorkflowId,
    index: u8,
) -> Result<Option<StoredWorkflowCandidate>, StorageError> {
    let row: Option<(String,String)> = connection.query_row("SELECT candidate_commitment,context_json FROM contract_workflow_candidates WHERE workflow_id=?1 AND step_index=?2 AND length(context_json)<=16384",params![id.0,index],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
    let Some((commitment, json)) = row else {
        let exists: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM contract_workflow_candidates WHERE workflow_id=?1 AND step_index=?2)",params![id.0,index],|row|row.get(0))?;
        return if exists {
            Err(StorageError::CorruptDatabase)
        } else {
            Ok(None)
        };
    };
    let candidate: StoredWorkflowCandidate =
        serde_json::from_str(&json).map_err(|_| StorageError::CorruptDatabase)?;
    if candidate.commitment()? != commitment
        || scope::bounded_json(&candidate, scope::MAX_CONTEXT_BYTES)? != json
    {
        return Err(StorageError::CorruptDatabase);
    }
    Ok(Some(candidate))
}
pub(super) fn validate_context(
    connection: &Connection,
    id: &ContractWorkflowId,
    steps: &[ContractWorkflowStep],
) -> Result<(), StorageError> {
    let scoped = load_scope(connection, id)?.is_some();
    for step in steps {
        let candidate = load_candidate(connection, id, step.index)?;
        let requires = scoped
            && matches!(
                step.state,
                ContractStepState::Dispatching
                    | ContractStepState::Submitted
                    | ContractStepState::ConfirmedUnverified
                    | ContractStepState::Verified
                    | ContractStepState::Uncertain
            );
        let observations = super::observation::load_observations(connection, id, step.index)?;
        if !observations.is_empty() && candidate.is_none() {
            return Err(StorageError::CorruptDatabase);
        }
        if let Some(candidate) = &candidate {
            for observation in &observations {
                if observation.candidate_commitment != candidate.commitment()?
                    || step.transaction_hash.as_deref()
                        != Some(observation.transaction_hash.as_str())
                    || observation
                        .results
                        .iter()
                        .map(|result| &result.observer)
                        .ne(candidate.observers.iter())
                {
                    return Err(StorageError::CorruptDatabase);
                }
            }
            if step.state == ContractStepState::Verified
                && !observations.iter().any(|observation| {
                    observation.commitment().ok().as_deref() == step.evidence_commitment.as_deref()
                })
            {
                return Err(StorageError::CorruptDatabase);
            }
        }
        if (requires && candidate.is_none())
            || (!scoped && candidate.is_some())
            || candidate
                .as_ref()
                .is_some_and(|candidate| candidate.intent_commitment != step.commitment)
        {
            return Err(StorageError::CorruptDatabase);
        }
    }
    Ok(())
}
