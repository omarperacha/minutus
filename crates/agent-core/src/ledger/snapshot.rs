use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{
    MAX_FAILURES, MAX_MUTATION_CHANGES, MAX_MUTATIONS, MAX_OBSERVATIONS, MAX_PARTIAL_OUTCOMES,
    MAX_PERMISSIONS, MAX_WORKSPACES,
};
use crate::{
    AgentSystemInstructionIdentity, ExecutionBudgets, ExecutionLedgerError, FactOrigin,
    LedgerNextAction, LedgerPermissionDecision, LedgerRequest, LedgerResource, MutationFact,
    ObservationFact, PartialOutcome, PermissionFact, UnresolvedFailure, WorkspaceResolution,
    ledger_types::{validate_id, validate_message},
};

pub(super) const LEDGER_VERSION: u16 = 4;
const MAX_SNAPSHOT_BYTES: usize = 1_048_576;

/// Persistable host-owned execution ledger state.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionLedgerSnapshot {
    pub(super) version: u16,
    pub(super) run_id: String,
    pub(super) request: LedgerRequest,
    pub(super) system_instructions: AgentSystemInstructionIdentity,
    pub(super) sequence: u64,
    pub(super) workspaces: BTreeMap<String, WorkspaceResolution>,
    pub(super) observations: Vec<ObservationFact>,
    pub(super) permissions: Vec<PermissionFact>,
    pub(super) mutations: Vec<MutationFact>,
    pub(super) unresolved_failures: Vec<UnresolvedFailure>,
    pub(super) partial_outcomes: Vec<PartialOutcome>,
    pub(super) budgets: ExecutionBudgets,
    pub(super) next_action: LedgerNextAction,
}

impl ExecutionLedgerSnapshot {
    /// Returns the run ID used as the persistence key.
    #[must_use]
    pub fn run_id(&self) -> &str {
        &self.run_id
    }
    /// Returns the owning conversation ID.
    #[must_use]
    pub fn conversation_id(&self) -> &str {
        self.request.conversation_id()
    }
    /// Returns the owning project ID.
    #[must_use]
    pub fn project_id(&self) -> &str {
        self.request.project_id()
    }
}

pub(super) fn validate_snapshot(
    snapshot: &ExecutionLedgerSnapshot,
) -> Result<(), ExecutionLedgerError> {
    if snapshot.version != LEDGER_VERSION {
        return Err(ExecutionLedgerError::InvalidRecord(
            "unsupported ledger version".to_owned(),
        ));
    }
    validate_id("ledger run ID", &snapshot.run_id)?;
    snapshot.request.validate()?;
    snapshot
        .system_instructions
        .validate()
        .map_err(|error| ExecutionLedgerError::InvalidRecord(error.to_string()))?;
    snapshot.next_action.validate()?;
    if snapshot.workspaces.len() > MAX_WORKSPACES
        || snapshot.observations.len() > MAX_OBSERVATIONS
        || snapshot.permissions.len() > MAX_PERMISSIONS
        || snapshot.mutations.len() > MAX_MUTATIONS
        || snapshot.unresolved_failures.len() > MAX_FAILURES
        || snapshot.partial_outcomes.len() > MAX_PARTIAL_OUTCOMES
    {
        return Err(ExecutionLedgerError::LimitExceeded);
    }
    for (alias, workspace) in &snapshot.workspaces {
        workspace.validate()?;
        if alias != workspace.alias() {
            return Err(ExecutionLedgerError::ScopeMismatch);
        }
    }
    let mut ids = BTreeSet::new();
    let mut sequences = BTreeSet::new();
    validate_observations(snapshot, &mut ids, &mut sequences)?;
    validate_mutations(snapshot, &mut ids, &mut sequences)?;
    validate_permissions(snapshot, &mut ids, &mut sequences)?;
    validate_failures(snapshot, &mut ids, &mut sequences)?;
    validate_partial_outcomes(snapshot, &mut ids, &mut sequences)?;
    validate_sequences(snapshot.sequence, &sequences)?;
    ensure_snapshot_size(snapshot)
}

fn validate_observations<'a>(
    snapshot: &'a ExecutionLedgerSnapshot,
    ids: &mut BTreeSet<&'a str>,
    sequences: &mut BTreeSet<u64>,
) -> Result<(), ExecutionLedgerError> {
    for fact in &snapshot.observations {
        fact.origin().validate()?;
        fact.resource().validate()?;
        validate_id("observation fact ID", fact.fact_id())?;
        validate_id("observation resource revision", fact.resource_revision())?;
        validate_id(
            "observation capability revision",
            fact.capability_revision(),
        )?;
        if fact.origin().result_id().is_none() {
            return Err(ExecutionLedgerError::InvalidRecord(
                "observation result provenance is missing".to_owned(),
            ));
        }
        fact.values().validate()?;
        validate_fact_scope(snapshot, fact.origin(), fact.resource())?;
        validate_fact_identity(fact.fact_id(), fact.sequence(), ids, sequences)?;
    }
    Ok(())
}

fn validate_mutations<'a>(
    snapshot: &'a ExecutionLedgerSnapshot,
    ids: &mut BTreeSet<&'a str>,
    sequences: &mut BTreeSet<u64>,
) -> Result<(), ExecutionLedgerError> {
    for mutation in &snapshot.mutations {
        mutation.origin().validate()?;
        validate_id("mutation ID", mutation.mutation_id())?;
        if mutation.origin().result_id().is_none() {
            return Err(ExecutionLedgerError::InvalidRecord(
                "mutation result provenance is missing".to_owned(),
            ));
        }
        if mutation.changes().is_empty() || mutation.changes().len() > MAX_MUTATION_CHANGES {
            return Err(ExecutionLedgerError::LimitExceeded);
        }
        for change in mutation.changes() {
            change.validate()?;
            validate_fact_scope(snapshot, mutation.origin(), change.resource())?;
        }
        validate_fact_identity(mutation.mutation_id(), mutation.sequence(), ids, sequences)?;
    }
    Ok(())
}

fn validate_permissions<'a>(
    snapshot: &'a ExecutionLedgerSnapshot,
    ids: &mut BTreeSet<&'a str>,
    sequences: &mut BTreeSet<u64>,
) -> Result<(), ExecutionLedgerError> {
    let mut lifecycle = BTreeMap::new();
    let mut permissions = snapshot.permissions.iter().collect::<Vec<_>>();
    permissions.sort_unstable_by_key(|permission| permission.sequence());
    for permission in permissions {
        permission.origin().validate()?;
        validate_origin_scope(snapshot, permission.origin())?;
        validate_id("permission fact ID", permission.fact_id())?;
        validate_id("permission ID", permission.permission_id())?;
        validate_id("permission tool name", permission.tool_name())?;
        permission.binding().validate()?;
        if permission.binding().call_id() != permission.origin().tool_call_id()
            || permission.binding().resource_scope().project_id() != snapshot.request.project_id()
        {
            return Err(ExecutionLedgerError::ScopeMismatch);
        }
        if permission.origin().result_id().is_some() {
            return Err(ExecutionLedgerError::InvalidRecord(
                "permission result provenance must be absent".to_owned(),
            ));
        }
        match lifecycle.get(permission.permission_id()) {
            None => {
                lifecycle.insert(permission.permission_id(), permission.decision());
            }
            Some(LedgerPermissionDecision::Pending)
                if permission.decision() != LedgerPermissionDecision::Pending =>
            {
                lifecycle.insert(permission.permission_id(), permission.decision());
            }
            Some(_) => {
                return Err(ExecutionLedgerError::InvalidRecord(
                    "permission lifecycle is inconsistent".to_owned(),
                ));
            }
        }
        validate_fact_identity(permission.fact_id(), permission.sequence(), ids, sequences)?;
    }
    Ok(())
}

fn validate_failures<'a>(
    snapshot: &'a ExecutionLedgerSnapshot,
    ids: &mut BTreeSet<&'a str>,
    sequences: &mut BTreeSet<u64>,
) -> Result<(), ExecutionLedgerError> {
    for failure in &snapshot.unresolved_failures {
        failure.origin().validate()?;
        validate_origin_scope(snapshot, failure.origin())?;
        validate_id("failure ID", failure.failure_id())?;
        validate_id("failure code", failure.code())?;
        validate_message("failure message", failure.message())?;
        if failure.origin().result_id().is_none() {
            return Err(ExecutionLedgerError::InvalidRecord(
                "failure result provenance is missing".to_owned(),
            ));
        }
        validate_fact_identity(failure.failure_id(), failure.sequence(), ids, sequences)?;
    }
    Ok(())
}

fn validate_partial_outcomes<'a>(
    snapshot: &'a ExecutionLedgerSnapshot,
    ids: &mut BTreeSet<&'a str>,
    sequences: &mut BTreeSet<u64>,
) -> Result<(), ExecutionLedgerError> {
    for outcome in &snapshot.partial_outcomes {
        outcome.origin().validate()?;
        validate_origin_scope(snapshot, outcome.origin())?;
        validate_id("partial outcome ID", outcome.outcome_id())?;
        validate_message("partial outcome summary", outcome.summary())?;
        if outcome.origin().result_id().is_none() {
            return Err(ExecutionLedgerError::InvalidRecord(
                "partial outcome result provenance is missing".to_owned(),
            ));
        }
        validate_fact_identity(outcome.outcome_id(), outcome.sequence(), ids, sequences)?;
    }
    Ok(())
}

fn validate_fact_identity<'a>(
    id: &'a str,
    sequence: u64,
    ids: &mut BTreeSet<&'a str>,
    sequences: &mut BTreeSet<u64>,
) -> Result<(), ExecutionLedgerError> {
    if !ids.insert(id) {
        return Err(ExecutionLedgerError::DuplicateFact(id.to_owned()));
    }
    if !sequences.insert(sequence) {
        return Err(ExecutionLedgerError::InvalidRecord(
            "ledger fact sequence is duplicated".to_owned(),
        ));
    }
    Ok(())
}

fn validate_sequences(
    expected: u64,
    sequences: &BTreeSet<u64>,
) -> Result<(), ExecutionLedgerError> {
    let fact_count =
        u64::try_from(sequences.len()).map_err(|_| ExecutionLedgerError::LimitExceeded)?;
    if fact_count != expected
        || sequences.first().is_some_and(|first| *first != 1)
        || sequences.last().is_some_and(|last| *last != expected)
    {
        return Err(ExecutionLedgerError::InvalidRecord(
            "ledger sequence is inconsistent".to_owned(),
        ));
    }
    Ok(())
}

fn validate_fact_scope(
    snapshot: &ExecutionLedgerSnapshot,
    origin: &FactOrigin,
    resource: &LedgerResource,
) -> Result<(), ExecutionLedgerError> {
    validate_origin_scope(snapshot, origin)?;
    let workspace = snapshot
        .workspaces
        .get(resource.workspace_alias())
        .ok_or_else(|| {
            ExecutionLedgerError::UnknownWorkspace(resource.workspace_alias().to_owned())
        })?;
    if workspace.capability_id() != resource.capability_id() {
        return Err(ExecutionLedgerError::ScopeMismatch);
    }
    Ok(())
}

fn validate_origin_scope(
    snapshot: &ExecutionLedgerSnapshot,
    origin: &FactOrigin,
) -> Result<(), ExecutionLedgerError> {
    if origin.request_id() != snapshot.request.request_id()
        || origin.project_id() != snapshot.request.project_id()
    {
        return Err(ExecutionLedgerError::ScopeMismatch);
    }
    Ok(())
}

pub(super) fn ensure_snapshot_size(
    snapshot: &ExecutionLedgerSnapshot,
) -> Result<(), ExecutionLedgerError> {
    let size = serde_json::to_vec(snapshot)
        .map_err(|error| ExecutionLedgerError::InvalidRecord(error.to_string()))?
        .len();
    if size > MAX_SNAPSHOT_BYTES {
        return Err(ExecutionLedgerError::LimitExceeded);
    }
    Ok(())
}
