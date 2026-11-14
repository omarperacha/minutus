use super::{
    ContractStepClaim, ContractStepState, ContractWorkflowId, ContractWorkflowStep, records,
};
use crate::{ApplicationStore, StorageError};
use ipc_types::{ConversationId, MessageId};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

impl ApplicationStore {
    /// Claims one host-created ordered workflow for the current persisted user
    /// turn. Shares the standalone transfer tombstone: aliases, revisions and new
    /// model call IDs cannot create a second workflow or transfer in that turn.
    /// Commitments must be derived by trusted host code from exact scope/steps.
    ///
    /// # Errors
    /// Returns validation/storage failure; `None` is not authority to proceed.
    pub fn create_contract_workflow(
        &mut self,
        conversation: &ConversationId,
        message: &MessageId,
        scope_commitment: &str,
        step_commitments: &[String],
    ) -> Result<Option<ContractWorkflowId>, StorageError> {
        records::validate_digest(scope_commitment)?;
        if step_commitments.is_empty() || step_commitments.len() > 16 {
            return Err(StorageError::InvalidRecord(
                "workflow must contain 1 to 16 ordered steps",
            ));
        }
        for commitment in step_commitments {
            records::validate_digest(commitment)?;
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(id) = super::scoped::create_manifest(
            &tx,
            conversation,
            message,
            scope_commitment,
            step_commitments,
            false,
        )?
        else {
            return Ok(None);
        };
        tx.commit()?;
        Ok(Some(id))
    }

    /// Looks up the existing host identity after restart; never creates a claim.
    ///
    /// # Errors
    /// Returns a storage failure.
    pub fn contract_workflow_for_turn(
        &self,
        conversation: &ConversationId,
        message: &MessageId,
    ) -> Result<Option<ContractWorkflowId>, StorageError> {
        self.connection
            .query_row(
                "SELECT id FROM contract_workflows WHERE conversation_id=?1 AND message_id=?2",
                params![conversation.0, message.0],
                |row| row.get(0).map(ContractWorkflowId),
            )
            .optional()
            .map_err(StorageError::from)
    }

    /// Returns ordered public status/evidence. Partial completion may leave
    /// allowances or other effects; nothing here automatically cleans them up.
    ///
    /// # Errors
    /// Fails on corrupt/unknown states or database failure.
    pub fn contract_workflow_steps(
        &self,
        id: &ContractWorkflowId,
    ) -> Result<Vec<ContractWorkflowStep>, StorageError> {
        records::load_workflow(&self.connection, id)
    }

    /// Consumes exactly the next ordered step once, after every prerequisite has
    /// host-verified postcondition evidence. Returns no reusable signing handle.
    /// Denial, cancellation, failure, restart or uncertainty block continuation.
    ///
    /// # Errors
    /// Returns storage failure; `None` means no step may proceed.
    pub fn claim_contract_step(
        &mut self,
        id: &ContractWorkflowId,
    ) -> Result<Option<ContractStepClaim>, StorageError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let steps = records::load_workflow(&tx, id)?;
        let current:bool=tx.query_row("SELECT EXISTS (SELECT 1 FROM contract_workflows w JOIN messages m ON m.id=w.message_id AND m.conversation_id=w.conversation_id AND m.role='user' WHERE w.id=?1 AND NOT EXISTS (SELECT 1 FROM messages newer WHERE newer.conversation_id=w.conversation_id AND newer.role='user' AND newer.position>m.position))",params![id.0],|row|row.get(0))?;
        if !current {
            return Ok(None);
        }
        let Some(step) = steps
            .into_iter()
            .find(|step| step.state != ContractStepState::Verified)
        else {
            return Ok(None);
        };
        if step.state != ContractStepState::Planned {
            return Ok(None);
        }
        let index = step.index;
        let commitment = step.commitment;
        tx.execute("UPDATE contract_workflow_steps SET state='reviewing' WHERE workflow_id=?1 AND step_index=?2 AND state='planned'",params![id.0,index])?;
        tx.commit()?;
        Ok(Some(ContractStepClaim {
            workflow: id.clone(),
            index,
            commitment,
        }))
    }

    /// Records a host-observed lifecycle transition using compare-and-swap.
    /// Dispatching must be durable before any bytes can reach a send transport.
    /// A receipt can reach `ConfirmedUnverified`, never `Verified` through this API.
    ///
    /// # Errors
    /// Rejects illegal transitions, missing/changed transaction hash or storage failure.
    pub fn record_contract_step(
        &mut self,
        claim: &ContractStepClaim,
        from: ContractStepState,
        to: ContractStepState,
        transaction_hash: Option<&str>,
    ) -> Result<bool, StorageError> {
        if !from.permits(to) {
            return Err(StorageError::InvalidRecord(
                "illegal contract step transition",
            ));
        }
        if let Some(hash) = transaction_hash {
            records::validate_hash(hash)?;
        }
        if matches!(
            to,
            ContractStepState::Submitted | ContractStepState::ConfirmedUnverified
        ) && transaction_hash.is_none()
        {
            return Err(StorageError::InvalidRecord("transaction hash required"));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let steps = records::load_workflow(&tx, &claim.workflow)?;
        if from == ContractStepState::Reviewing
            && to == ContractStepState::Dispatching
            && super::scoped::load_scope(&tx, &claim.workflow)?.is_some()
        {
            return Err(StorageError::InvalidRecord(
                "scoped dispatch requires atomic nonce admission",
            ));
        }
        if from == ContractStepState::Reviewing && to == ContractStepState::Dispatching {
            let current:bool=tx.query_row("SELECT EXISTS (SELECT 1 FROM contract_workflows w JOIN messages m ON m.id=w.message_id AND m.conversation_id=w.conversation_id AND m.role='user' WHERE w.id=?1 AND NOT EXISTS (SELECT 1 FROM messages newer WHERE newer.conversation_id=w.conversation_id AND newer.role='user' AND newer.position>m.position))",params![claim.workflow.0],|row|row.get(0))?;
            if !current
                || steps
                    .iter()
                    .take(usize::from(claim.index))
                    .any(|step| step.state != ContractStepState::Verified)
            {
                return Ok(false);
            }
        }
        let changed=tx.execute("UPDATE contract_workflow_steps SET state=?1,transaction_hash=COALESCE(transaction_hash,?2),evidence_commitment=NULL
            WHERE workflow_id=?3 AND step_index=?4 AND commitment=?5 AND state=?6 AND (transaction_hash IS NULL OR ?2 IS NULL OR transaction_hash=?2)",params![to.name(),transaction_hash,claim.workflow.0,claim.index,claim.commitment,from.name()])?==1;
        if changed && to == ContractStepState::Uncertain {
            // Previously claimed native authority cannot survive a prerequisite
            // invalidation, even if that prerequisite is later re-verified.
            tx.execute("UPDATE contract_workflow_steps SET state='interrupted' WHERE workflow_id=?1 AND step_index>?2 AND state IN ('planned','reviewing')",params![claim.workflow.0,claim.index])?;
        }
        records::load_workflow(&tx, &claim.workflow)?;
        tx.commit()?;
        Ok(changed)
    }

    /// Records a separately established postcondition commitment for a confirmed
    /// step. The trusted host must verify canonical receipt AND intended effects;
    /// neither a model claim, log label nor receipt success alone is sufficient.
    /// This dormant API is not wired to model tools or transaction execution.
    ///
    /// # Errors
    /// Returns invalid evidence or storage failure; false means stale/ineligible.
    pub fn verify_contract_step_postcondition(
        &mut self,
        claim: &ContractStepClaim,
        transaction_hash: &str,
        evidence_commitment: &str,
    ) -> Result<bool, StorageError> {
        records::validate_hash(transaction_hash)?;
        records::validate_digest(evidence_commitment)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        records::load_workflow(&tx, &claim.workflow)?;
        if super::scoped::load_scope(&tx, &claim.workflow)?.is_some() {
            return Err(StorageError::InvalidRecord(
                "scoped postconditions require complete verified observation context",
            ));
        }
        let changed=tx.execute("UPDATE contract_workflow_steps SET state='verified' ,evidence_commitment=?1 WHERE workflow_id=?2 AND step_index=?3 AND commitment=?4 AND state='confirmed_unverified' AND transaction_hash=?5",params![evidence_commitment,claim.workflow.0,claim.index,claim.commitment,transaction_hash])?==1;
        records::load_workflow(&tx, &claim.workflow)?;
        tx.commit()?;
        Ok(changed)
    }

    /// Conservative restart handling: never revive review authority. A possible
    /// send remains uncertain, and a mere confirmed receipt remains unverified.
    /// No API here resumes an interrupted workflow or retries/cleans allowances.
    /// Call only during exclusive startup before new work is admitted.
    ///
    /// # Errors
    /// Returns storage failure; callers must keep writes unavailable on failure.
    pub fn interrupt_contract_workflows_after_restart(&mut self) -> Result<(), StorageError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("UPDATE contract_workflow_steps SET state='interrupted' WHERE state IN ('planned','reviewing')",[])?;
        tx.execute("UPDATE contract_workflow_steps SET state='uncertain' WHERE state IN ('dispatching','submitted')",[])?;
        tx.commit()?;
        Ok(())
    }
}
