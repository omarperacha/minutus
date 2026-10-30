use super::{
    ContractStepClaim, ContractWorkflowId,
    binding::{StoredContractObserver, StoredObserverPredicate, StoredPredicateComparison},
    records, scope, scoped,
};
use crate::{ApplicationStore, StorageError};
use rusqlite::{Connection, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

/// One structural observed value. Only a separate qualified proof can trust it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoredObserverResult {
    /// Exact prepared observer identity/profile/predicate.
    pub observer: StoredContractObserver,
    /// Canonical unsigned decimal, or a complete application storage word.
    pub value: String,
}
impl StoredObserverResult {
    fn validate(&self) -> Result<(), StorageError> {
        self.observer.validate()?;
        if matches!(
            self.observer.predicate,
            StoredObserverPredicate::ApplicationWord { .. }
        ) {
            records::validate_hash(&self.value)
        } else {
            scope::decimal(&self.value)
        }
    }
    fn satisfies(&self) -> bool {
        match &self.observer.predicate {
            StoredObserverPredicate::Allowance { amount, .. } => &self.value == amount,
            StoredObserverPredicate::ApplicationWord { value, .. } => &self.value == value,
            StoredObserverPredicate::TokenBalance {
                comparison, amount, ..
            } => {
                let ordering = (self.value.len(), &self.value).cmp(&(amount.len(), amount));
                match comparison {
                    StoredPredicateComparison::Equal => ordering.is_eq(),
                    StoredPredicateComparison::AtLeast => !ordering.is_lt(),
                    StoredPredicateComparison::AtMost => !ordering.is_gt(),
                }
            }
        }
    }
}

/// Bounded durable receipt/state context. Deserialisation never verifies it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoredWorkflowObservation {
    /// Exact prepared context to which this observation relates.
    pub candidate_commitment: String,
    /// Exact signed transaction being reconciled.
    pub transaction_hash: String,
    /// Canonical receipt block height.
    pub receipt_block_number: String,
    /// Canonical receipt block identity, not finality.
    pub receipt_block_hash: String,
    /// Exact block of the independent state assertions.
    pub state_block_number: String,
    /// Exact state block identity; may be later than the receipt block.
    pub state_block_hash: String,
    /// Separately verified receipt/transaction correspondence commitment.
    pub receipt_commitment: String,
    /// Separately verified state/source/profile proof commitment.
    pub proof_commitment: String,
    /// Complete ordered prepared observers and actual values.
    pub results: Vec<StoredObserverResult>,
}
impl StoredWorkflowObservation {
    /// Commits structural context without declaring it trusted or satisfied.
    ///
    /// # Errors
    /// Rejects malformed, oversized or incomplete observation records.
    pub fn commitment(&self) -> Result<String, StorageError> {
        self.validate()?;
        scope::digest(b"minutus.contract-workflow-observation.v1", self)
    }
    fn validate(&self) -> Result<(), StorageError> {
        for hash in [
            &self.transaction_hash,
            &self.receipt_block_hash,
            &self.state_block_hash,
        ] {
            records::validate_hash(hash)?;
        }
        for digest in [
            &self.candidate_commitment,
            &self.receipt_commitment,
            &self.proof_commitment,
        ] {
            records::validate_digest(digest)?;
        }
        scope::decimal(&self.receipt_block_number)?;
        scope::decimal(&self.state_block_number)?;
        if (self.state_block_number.len(), &self.state_block_number)
            < (self.receipt_block_number.len(), &self.receipt_block_number)
            || self.results.is_empty()
            || self.results.len() > 16
        {
            return scope::invalid("invalid observation height/count");
        }
        for result in &self.results {
            result.validate()?;
        }
        scope::bounded_json(self, scope::MAX_CONTEXT_BYTES).map(|_| ())
    }
}

impl ApplicationStore {
    /// Records structural observation context, including unmet postconditions.
    /// This method never credits a successful task outcome or releases a step.
    ///
    /// # Errors
    /// Rejects changed candidate/observers/hash or bounded storage failure.
    pub fn record_contract_step_observation(
        &mut self,
        claim: &ContractStepClaim,
        observation: &StoredWorkflowObservation,
    ) -> Result<bool, StorageError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        validate_binding(&tx, claim, observation)?;
        let changed = insert_observation(&tx, claim, observation)?;
        tx.commit()?;
        Ok(changed)
    }
    /// Persists a host-verified predicate after the application has checked its
    /// opaque receipt/state proof. This storage API validates structural binding,
    /// not cryptographic truth; it must never be exposed to model/IPC callers.
    ///
    /// # Errors
    /// Rejects unmet assertions, changed context, stale state or storage failure.
    pub fn apply_verified_contract_step_observation(
        &mut self,
        claim: &ContractStepClaim,
        observation: &StoredWorkflowObservation,
    ) -> Result<bool, StorageError> {
        let evidence = observation.commitment()?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        validate_binding(&tx, claim, observation)?;
        if observation.results.iter().any(|result| !result.satisfies()) {
            return scope::invalid("postcondition assertion is unmet");
        }
        insert_observation(&tx, claim, observation)?;
        let changed = tx.execute("UPDATE contract_workflow_steps SET state='verified',evidence_commitment=?1 WHERE workflow_id=?2 AND step_index=?3 AND commitment=?4 AND state='confirmed_unverified' AND transaction_hash=?5",params![evidence,claim.workflow.0,claim.index,claim.commitment,observation.transaction_hash])? == 1;
        records::load_workflow(&tx, &claim.workflow)?;
        tx.commit()?;
        Ok(changed)
    }
    /// Refreshes an already verified step using a newly checked opaque certificate
    /// consumed by `AppCore`. This cannot resurrect uncertain or reorganised state.
    ///
    /// # Errors
    /// Rejects changed binding, unmet assertions, non-verified state or storage faults.
    pub fn refresh_verified_contract_step(
        &mut self,
        id: &ContractWorkflowId,
        index: u8,
        intent_commitment: &str,
        observation: &StoredWorkflowObservation,
    ) -> Result<(), StorageError> {
        let evidence = observation.commitment()?;
        let claim = ContractStepClaim {
            workflow: id.clone(),
            index,
            commitment: intent_commitment.into(),
        };
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        validate_binding(&tx, &claim, observation)?;
        if observation.results.iter().any(|result| !result.satisfies()) {
            return scope::invalid("refreshed postcondition assertion is unmet");
        }
        let changed = tx.execute("UPDATE contract_workflow_steps SET evidence_commitment=?1 WHERE workflow_id=?2 AND step_index=?3 AND commitment=?4 AND state='verified' AND transaction_hash=?5", params![evidence,id.0,index,intent_commitment,observation.transaction_hash])? == 1;
        if !changed {
            return scope::invalid("verified prerequisite was invalidated");
        }
        insert_observation(&tx, &claim, observation)?;
        records::load_workflow(&tx, id)?;
        tx.commit()?;
        Ok(())
    }
    /// Loads historical bounded receipt/state contexts; none recreates authority.
    ///
    /// # Errors
    /// Rejects malformed or modified persisted observations.
    pub fn contract_step_observations(
        &self,
        id: &ContractWorkflowId,
        index: u8,
    ) -> Result<Vec<StoredWorkflowObservation>, StorageError> {
        records::load_workflow(&self.connection, id)?;
        load_observations(&self.connection, id, index)
    }
}
fn validate_binding(
    connection: &Connection,
    claim: &ContractStepClaim,
    observation: &StoredWorkflowObservation,
) -> Result<(), StorageError> {
    observation.validate()?;
    let steps = records::load_workflow(connection, &claim.workflow)?;
    let step = steps
        .get(usize::from(claim.index))
        .ok_or(StorageError::CorruptDatabase)?;
    let candidate = scoped::load_candidate(connection, &claim.workflow, claim.index)?.ok_or(
        StorageError::InvalidRecord("prepared candidate context required"),
    )?;
    if step.commitment != claim.commitment
        || step.transaction_hash.as_deref() != Some(observation.transaction_hash.as_str())
        || candidate.commitment()? != observation.candidate_commitment
        || observation
            .results
            .iter()
            .map(|result| &result.observer)
            .ne(candidate.observers.iter())
    {
        return scope::invalid("observation differs from exact workflow binding");
    }
    Ok(())
}
fn insert_observation(
    connection: &Connection,
    claim: &ContractStepClaim,
    observation: &StoredWorkflowObservation,
) -> Result<bool, StorageError> {
    let commitment = observation.commitment()?;
    let json = scope::bounded_json(observation, scope::MAX_CONTEXT_BYTES)?;
    let known:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM contract_workflow_observations WHERE workflow_id=?1 AND step_index=?2 AND evidence_commitment=?3)",params![claim.workflow.0,claim.index,commitment],|row|row.get(0))?;
    if known {
        return Ok(false);
    }
    let count:u32=connection.query_row("SELECT count(*) FROM contract_workflow_observations WHERE workflow_id=?1 AND step_index=?2",params![claim.workflow.0,claim.index],|row|row.get(0))?;
    if count >= 32 {
        return scope::invalid("workflow observation history limit reached");
    }
    Ok(connection.execute("INSERT INTO contract_workflow_observations(workflow_id,step_index,evidence_commitment,observation_json) VALUES (?1,?2,?3,?4)",params![claim.workflow.0,claim.index,commitment,json])?==1)
}
pub(super) fn load_observations(
    connection: &Connection,
    id: &ContractWorkflowId,
    index: u8,
) -> Result<Vec<StoredWorkflowObservation>, StorageError> {
    let mut query=connection.prepare("SELECT evidence_commitment,observation_json FROM contract_workflow_observations WHERE workflow_id=?1 AND step_index=?2 ORDER BY id LIMIT 33")?;
    let mut rows = query.query(params![id.0, index])?;
    let mut result = Vec::new();
    while let Some(row) = rows.next()? {
        let commitment: String = row.get(0)?;
        let json: String = row.get(1)?;
        if json.len() > scope::MAX_CONTEXT_BYTES || result.len() >= 32 {
            return Err(StorageError::CorruptDatabase);
        }
        let observation: StoredWorkflowObservation =
            serde_json::from_str(&json).map_err(|_| StorageError::CorruptDatabase)?;
        if observation.commitment()? != commitment
            || scope::bounded_json(&observation, scope::MAX_CONTEXT_BYTES)? != json
        {
            return Err(StorageError::CorruptDatabase);
        }
        result.push(observation);
    }
    Ok(result)
}
