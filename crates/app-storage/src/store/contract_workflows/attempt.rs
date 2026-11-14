//! Reserve the shared durable turn before native unlock, then attach scope once.
use super::{ContractWorkflowId, StoredWorkflowDeclaration, StoredWorkflowScope, scope, scoped};
use crate::{ApplicationStore, StorageError};
use ipc_types::{ConversationId, MessageId};
use rusqlite::{Transaction, TransactionBehavior, params};
use std::sync::Arc;

/// One newly persisted contract attempt bound to its exact live storage owner.
/// No IDs, existing tombstone or reopened store can reconstruct this value.
/// Dropping it never refunds the shared transfer/workflow attempt.
#[must_use = "dropping this claim leaves the turn's durable attempt consumed"]
pub struct ContractWorkflowAttemptClaim {
    owner: Arc<()>,
    conversation: ConversationId,
    message: MessageId,
}
impl ApplicationStore {
    /// Reserves the current persisted user turn before any wallet prompt.
    /// No selected account, workflow scope, approval or signing handle is needed.
    ///
    /// # Errors
    /// Rejects unavailable storage; None means an invalid/stale/already-used turn.
    pub fn claim_contract_workflow_attempt(
        &self,
        conversation: &ConversationId,
        message: &MessageId,
    ) -> Result<Option<ContractWorkflowAttemptClaim>, StorageError> {
        if !self.claim_chat_transfer(conversation, message)? {
            return Ok(None);
        }
        Ok(Some(ContractWorkflowAttemptClaim {
            owner: Arc::clone(&self.contract_attempt_owner),
            conversation: conversation.clone(),
            message: message.clone(),
        }))
    }
    /// Consumes a live pre-unlock claim and atomically installs exact v1 scope.
    /// The claim is spent on stale turns, validation failure and storage error.
    ///
    /// # Errors
    /// Rejects another/reopened storage owner, invalid scope or storage failure.
    pub fn attach_scoped_contract_workflow(
        &mut self,
        attempt: ContractWorkflowAttemptClaim,
        scope: &StoredWorkflowScope,
    ) -> Result<Option<ContractWorkflowId>, StorageError> {
        require_owner(&self.contract_attempt_owner, &attempt)?;
        let commitment = scope.commitment()?;
        let steps = scope.step_commitments()?;
        let json = scope::bounded_json(scope, scope::MAX_SCOPE_BYTES)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(id) = attach_manifest(&tx, attempt, &commitment, &steps)? else {
            return Ok(None);
        };
        tx.execute("INSERT INTO contract_workflow_scopes(workflow_id,format_version,scope_json) VALUES (?1,1,?2)", params![id.0,json])?;
        tx.commit()?;
        Ok(Some(id))
    }
    /// Consumes a live pre-unlock claim and atomically installs exact v2 intent.
    /// The claim grants no source/effect, approval, signing or nonce capability.
    ///
    /// # Errors
    /// Rejects another/reopened storage owner, invalid intent or storage failure.
    pub fn attach_declared_contract_workflow(
        &mut self,
        attempt: ContractWorkflowAttemptClaim,
        declaration: &StoredWorkflowDeclaration,
    ) -> Result<Option<ContractWorkflowId>, StorageError> {
        require_owner(&self.contract_attempt_owner, &attempt)?;
        let commitment = declaration.commitment()?;
        let steps = declaration.step_commitments()?;
        let json = scope::bounded_json(declaration, scope::MAX_SCOPE_BYTES)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(id) = attach_manifest(&tx, attempt, &commitment, &steps)? else {
            return Ok(None);
        };
        tx.execute("INSERT INTO contract_workflow_declarations(workflow_id,format_version,declaration_json) VALUES (?1,2,?2)", params![id.0,json])?;
        tx.commit()?;
        Ok(Some(id))
    }
}
fn require_owner(
    owner: &Arc<()>,
    attempt: &ContractWorkflowAttemptClaim,
) -> Result<(), StorageError> {
    if !Arc::ptr_eq(owner, &attempt.owner) {
        return Err(StorageError::InvalidRecord(
            "workflow attempt belongs to another storage owner",
        ));
    }
    Ok(())
}
fn attach_manifest(
    tx: &Transaction<'_>,
    attempt: ContractWorkflowAttemptClaim,
    commitment: &str,
    steps: &[String],
) -> Result<Option<ContractWorkflowId>, StorageError> {
    let ContractWorkflowAttemptClaim {
        conversation,
        message,
        ..
    } = attempt;
    // This transaction serialises the current-turn and existing-workflow checks
    // with all manifest/step/scope inserts. Existence alone never mints a claim.
    let current: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM messages m JOIN chat_transfer_attempts a ON a.conversation_id=m.conversation_id AND a.message_id=m.id WHERE m.id=?2 AND m.conversation_id=?1 AND m.role='user' AND NOT EXISTS(SELECT 1 FROM messages newer WHERE newer.conversation_id=?1 AND newer.role='user' AND newer.position>m.position) AND NOT EXISTS(SELECT 1 FROM contract_workflows w WHERE w.conversation_id=?1 AND w.message_id=?2))",params![conversation.0,message.0],|row|row.get(0))?;
    if !current {
        return Ok(None);
    }
    scoped::insert_manifest(tx, &conversation, &message, commitment, steps, true).map(Some)
}

#[cfg(test)]
mod tests;
