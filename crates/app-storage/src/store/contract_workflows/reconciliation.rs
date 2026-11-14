//! Exact stored-hash observation cannot recreate a workflow or nonce claim.
use super::{
    ContractStepState, ContractWorkflowId, StoredGenericWorkflowPreparation,
    StoredWorkflowDeclaration, generic, records, scope,
};
use crate::{ApplicationStore, StorageError};
use ipc_types::ConversationId;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

/// Validated journal snapshot for read-only reconciliation, never execution authority.
pub struct StoredContractReconciliation {
    id: ContractWorkflowId,
    index: u8,
    hash: String,
    declaration: StoredWorkflowDeclaration,
    preparation: StoredGenericWorkflowPreparation,
}
impl StoredContractReconciliation {
    /// Immutable declaration whose transaction was already dispatched.
    #[must_use]
    pub const fn declaration(&self) -> &StoredWorkflowDeclaration {
        &self.declaration
    }
    /// Exact original signing/body binding; descriptive, not a signing capability.
    #[must_use]
    pub const fn preparation(&self) -> &StoredGenericWorkflowPreparation {
        &self.preparation
    }
    /// Exact known signed transaction identity.
    #[must_use]
    pub fn transaction_hash(&self) -> &str {
        &self.hash
    }
    /// Original ordered step position.
    #[must_use]
    pub const fn index(&self) -> u8 {
        self.index
    }
}
impl ApplicationStore {
    /// Finds only an existing dispatched declared step in this conversation.
    /// Unknown/cross-conversation hashes never create an RPC target or a claim.
    /// # Errors
    /// Rejects malformed hashes, duplicate identities and corrupt journal binding.
    pub fn contract_reconciliation(
        &self,
        conversation: &ConversationId,
        hash: &str,
    ) -> Result<Option<StoredContractReconciliation>, StorageError> {
        records::validate_hash(hash)?;
        let mut query = self.connection.prepare("SELECT s.workflow_id,s.step_index FROM contract_workflow_steps s JOIN contract_workflows w ON w.id=s.workflow_id JOIN contract_workflow_declarations d ON d.workflow_id=w.id WHERE w.conversation_id=?1 AND s.transaction_hash=?2 LIMIT 2")?;
        let matches = query
            .query_map(params![conversation.0, hash], |row| {
                Ok((ContractWorkflowId(row.get(0)?), row.get::<_, u8>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let [(id, index)] = matches.as_slice() else {
            return if matches.is_empty() {
                Ok(None)
            } else {
                Err(StorageError::CorruptDatabase)
            };
        };
        load(&self.connection, id, *index, hash).map(Some)
    }

    /// Records receipt inclusion only after the application consumes a fresh
    /// sealed chain certificate. It cannot mark a task verified or resume work.
    /// # Errors
    /// Rejects changed binding, invalid receipt facts or storage failure.
    pub fn record_contract_reconciliation(
        &mut self,
        original: &StoredContractReconciliation,
        succeeded: bool,
        block_number: &str,
        block_hash: &str,
        observed_at: &str,
    ) -> Result<ContractStepState, StorageError> {
        scope::decimal(block_number)?;
        records::validate_hash(block_hash)?;
        if observed_at.is_empty()
            || observed_at.len() > 64
            || observed_at.chars().any(char::is_control)
        {
            return scope::invalid("invalid reconciliation timestamp");
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = load(&tx, &original.id, original.index, &original.hash)?;
        if current.declaration != original.declaration
            || current.preparation != original.preparation
        {
            return scope::invalid("reconciliation journal binding changed");
        }
        // Receipt-only reconciliation cannot revalidate a selected completion
        // certificate, including one whose later observation block reorganised.
        // Immutable historical evidence rows remain available as old records.
        let state = if succeeded {
            ContractStepState::ConfirmedUnverified
        } else {
            ContractStepState::Failed
        };
        // Reconciliation never restores native authority, even when an old
        // uncertain step acquires a successful receipt. Possible sends survive.
        tx.execute("UPDATE contract_workflow_steps SET state='interrupted' WHERE workflow_id=?1 AND step_index>?2 AND state IN ('planned','reviewing')",params![original.id.0,original.index])?;
        tx.execute("UPDATE contract_workflow_steps SET state=?1,evidence_commitment=NULL WHERE workflow_id=?2 AND step_index=?3 AND transaction_hash=?4",params![state.name(),original.id.0,original.index,original.hash])?;
        // This atomic update changes observation fields only. Nonce ownership,
        // dispatch phase, candidate/body binding and attempt tombstones survive.
        let changed=tx.execute("UPDATE evm_transactions SET state=?1,block_number=?2,block_hash=?3,updated_at=?4 WHERE transaction_hash=?5 AND network=?6 AND lower(sender)=?7 AND nonce=?8 AND candidate_digest=?9",params![if succeeded {"confirmed"} else {"failed"},block_number,block_hash,observed_at,original.hash,current.declaration.network,current.declaration.account,current.preparation.nonce,current.preparation.candidate_digest])?;
        if changed != 1 {
            return Err(StorageError::CorruptDatabase);
        }
        records::load_workflow(&tx, &original.id)?;
        tx.commit()?;
        Ok(state)
    }
}
fn load(
    connection: &Connection,
    id: &ContractWorkflowId,
    index: u8,
    hash: &str,
) -> Result<StoredContractReconciliation, StorageError> {
    let steps = records::load_workflow(connection, id)?;
    let step = steps
        .get(usize::from(index))
        .ok_or(StorageError::CorruptDatabase)?;
    if step.transaction_hash.as_deref() != Some(hash)
        || !matches!(
            step.state,
            ContractStepState::Dispatching
                | ContractStepState::Submitted
                | ContractStepState::ConfirmedUnverified
                | ContractStepState::Verified
                | ContractStepState::Uncertain
                | ContractStepState::Failed
        )
    {
        return scope::invalid("reconciliation requires an existing possible submission");
    }
    let declaration =
        generic::load_declaration(connection, id)?.ok_or(StorageError::CorruptDatabase)?;
    let preparation =
        generic::load_preparation(connection, id, index)?.ok_or(StorageError::CorruptDatabase)?;
    let known:Option<i64>=connection.query_row("SELECT 1 FROM evm_transactions e JOIN evm_nonce_claims n ON n.signed_hash=e.transaction_hash JOIN evm_transfer_dispatches d ON d.transaction_hash=e.transaction_hash AND d.reservation_id=n.owner_token WHERE e.transaction_hash=?1 AND e.network=?2 AND lower(e.sender)=?3 AND e.nonce=?4 AND e.candidate_digest=?5 AND n.network=e.network AND n.sender=lower(e.sender) AND n.nonce=e.nonce AND n.candidate_digest=e.candidate_digest AND n.phase='possible_send' AND d.phase IN ('dispatched','acknowledged')",params![hash,declaration.network,declaration.account,preparation.nonce,preparation.candidate_digest],|row|row.get(0)).optional()?;
    if known != Some(1) {
        return scope::invalid("reconciliation differs from durable dispatch binding");
    }
    Ok(StoredContractReconciliation {
        id: id.clone(),
        index,
        hash: hash.into(),
        declaration,
        preparation,
    })
}

#[cfg(test)]
mod tests;
