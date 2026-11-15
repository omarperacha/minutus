//! Generic declarations and immutable preparation persistence.
use super::{
    ContractStepClaim, ContractStepState, ContractWorkflowId, StoredWorkflowDeclaration,
    generic_records::StoredGenericWorkflowPreparation, records, scope, scoped,
};
use crate::{ApplicationStore, StorageError};
use ipc_types::{ConversationId, MessageId};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

impl ApplicationStore {
    /// Starts one declared workflow for the current persisted user turn.
    ///
    /// # Errors
    /// Rejects malformed declarations, stale turns or corrupt/unavailable storage.
    pub fn create_declared_contract_workflow(
        &mut self,
        conversation: &ConversationId,
        message: &MessageId,
        declaration: &StoredWorkflowDeclaration,
    ) -> Result<Option<ContractWorkflowId>, StorageError> {
        let commitment = declaration.commitment()?;
        let steps = declaration.step_commitments()?;
        let json = scope::bounded_json(declaration, scope::MAX_SCOPE_BYTES)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(id) =
            scoped::create_manifest(&tx, conversation, message, &commitment, &steps, true)?
        else {
            return Ok(None);
        };
        tx.execute("INSERT INTO contract_workflow_declarations(workflow_id,format_version,declaration_json) VALUES (?1,2,?2)", params![id.0,json])?;
        tx.commit()?;
        Ok(Some(id))
    }

    /// Loads structural declarations, never source/effect authority.
    ///
    /// # Errors
    /// Rejects modified, unsupported or unavailable records.
    pub fn contract_workflow_declaration(
        &self,
        id: &ContractWorkflowId,
    ) -> Result<Option<StoredWorkflowDeclaration>, StorageError> {
        load_declaration(&self.connection, id)
    }

    /// Binds the one exact fresh candidate after the step attempt is consumed.
    ///
    /// # Errors
    /// Rejects rebinding, retired authority or mismatched structural context.
    pub fn bind_generic_contract_preparation(
        &mut self,
        claim: &ContractStepClaim,
        preparation: &StoredGenericWorkflowPreparation,
    ) -> Result<bool, StorageError> {
        let commitment = preparation.commitment()?;
        let json = scope::bounded_json(preparation, scope::MAX_CONTEXT_BYTES)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let steps = records::load_workflow(&tx, &claim.workflow)?;
        let declaration = load_declaration(&tx, &claim.workflow)?
            .ok_or(StorageError::InvalidRecord("generic declaration required"))?;
        let step = steps
            .get(usize::from(claim.index))
            .ok_or(StorageError::CorruptDatabase)?;
        if step.state != ContractStepState::Reviewing
            || step.commitment != claim.commitment
            || !scoped::current_turn(&tx, &claim.workflow)?
        {
            return Ok(false);
        }
        if preparation.scope_commitment != declaration.commitment()?
            || preparation.intent_commitment != step.commitment
        {
            return scope::invalid("generic preparation differs from immutable declaration");
        }
        let inserted = tx.execute("INSERT INTO contract_workflow_preparations(workflow_id,step_index,preparation_commitment,preparation_json) VALUES (?1,?2,?3,?4) ON CONFLICT DO NOTHING",params![claim.workflow.0,claim.index,commitment,json])? == 1;
        tx.commit()?;
        Ok(inserted)
    }

    /// Reads preparation facts without recreating an approval or fresh capability.
    ///
    /// # Errors
    /// Rejects corrupt records or storage failure.
    pub fn generic_contract_preparation(
        &self,
        id: &ContractWorkflowId,
        index: u8,
    ) -> Result<Option<StoredGenericWorkflowPreparation>, StorageError> {
        records::load_workflow(&self.connection, id)?;
        load_preparation(&self.connection, id, index)
    }
}

pub(super) fn load_declaration(
    connection: &Connection,
    id: &ContractWorkflowId,
) -> Result<Option<StoredWorkflowDeclaration>, StorageError> {
    let row:Option<(i64,String,String,String,i64)>=connection.query_row("SELECT d.format_version,d.declaration_json,w.scope_commitment,w.step_commitments_json,w.scope_format FROM contract_workflow_declarations d JOIN contract_workflows w ON w.id=d.workflow_id WHERE d.workflow_id=?1",params![id.0],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))).optional()?;
    let Some((version, json, commitment, manifest, scoped)) = row else {
        return Ok(None);
    };
    if version != 2 || scoped != 1 || json.len() > scope::MAX_SCOPE_BYTES || manifest.len() > 2048 {
        return Err(StorageError::CorruptDatabase);
    }
    let declaration: StoredWorkflowDeclaration =
        serde_json::from_str(&json).map_err(|_| StorageError::CorruptDatabase)?;
    let manifest: Vec<String> =
        serde_json::from_str(&manifest).map_err(|_| StorageError::CorruptDatabase)?;
    if declaration.commitment()? != commitment
        || declaration.step_commitments()? != manifest
        || scope::bounded_json(&declaration, scope::MAX_SCOPE_BYTES)? != json
    {
        return Err(StorageError::CorruptDatabase);
    }
    Ok(Some(declaration))
}
pub(super) fn load_preparation(
    connection: &Connection,
    id: &ContractWorkflowId,
    index: u8,
) -> Result<Option<StoredGenericWorkflowPreparation>, StorageError> {
    let row:Option<(String,String)>=connection.query_row("SELECT preparation_commitment,preparation_json FROM contract_workflow_preparations WHERE workflow_id=?1 AND step_index=?2",params![id.0,index],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
    let Some((commitment, json)) = row else {
        return Ok(None);
    };
    if json.len() > scope::MAX_CONTEXT_BYTES {
        return Err(StorageError::CorruptDatabase);
    }
    let preparation: StoredGenericWorkflowPreparation =
        serde_json::from_str(&json).map_err(|_| StorageError::CorruptDatabase)?;
    if preparation.commitment()? != commitment
        || scope::bounded_json(&preparation, scope::MAX_CONTEXT_BYTES)? != json
    {
        return Err(StorageError::CorruptDatabase);
    }
    Ok(Some(preparation))
}
