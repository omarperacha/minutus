use std::collections::BTreeMap;

mod snapshot;

pub use snapshot::ExecutionLedgerSnapshot;
use snapshot::{LEDGER_VERSION, ensure_snapshot_size, validate_snapshot};

use crate::{
    AgentSystemInstructionIdentity, ExecutionBudgets, ExecutionLedgerError, FactOrigin,
    LedgerCapability, LedgerNextAction, LedgerPermissionDecision, LedgerRequest, LedgerResource,
    ModelFailureFact, ModelLedgerProjection, ModelMutationChange, ModelMutationFact,
    ModelObservationFact, ModelPartialOutcome, ModelPermissionFact, ModelWorkspaceFact,
    MutationChange, MutationFact, NewMutationChange, NewObservation, ObservationFact,
    PartialOutcome, PermissionBinding, PermissionFact, ReuseContext, ToolClassification,
    ToolOutcome, ToolResultEnvelope, UnresolvedFailure, ValidatedToolRequest, WorkspaceResolution,
    ledger_types::{
        FactParts, MutationChangeParts, validate_digest, validate_id, validate_message,
        validate_relative_path,
    },
};

const MAX_WORKSPACES: usize = 16;
const MAX_OBSERVATIONS: usize = 256;
const MAX_PERMISSIONS: usize = 128;
const MAX_MUTATIONS: usize = 64;
const MAX_MUTATION_CHANGES: usize = 32;
const MAX_FAILURES: usize = 128;
const MAX_PARTIAL_OUTCOMES: usize = 128;

/// Trusted compact facts for one active agent request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionLedger {
    run_id: String,
    request: LedgerRequest,
    system_instructions: AgentSystemInstructionIdentity,
    sequence: u64,
    workspaces: BTreeMap<String, WorkspaceResolution>,
    observations: Vec<ObservationFact>,
    permissions: Vec<PermissionFact>,
    mutations: Vec<MutationFact>,
    unresolved_failures: Vec<UnresolvedFailure>,
    partial_outcomes: Vec<PartialOutcome>,
    budgets: ExecutionBudgets,
    next_action: LedgerNextAction,
}

impl ExecutionLedger {
    /// Creates a new ledger for one exact request and project scope.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed identity or next-action state.
    pub fn new(
        run_id: impl Into<String>,
        request: LedgerRequest,
        system_instructions: AgentSystemInstructionIdentity,
        budgets: ExecutionBudgets,
        next_action: LedgerNextAction,
    ) -> Result<Self, ExecutionLedgerError> {
        let run_id = run_id.into();
        validate_id("ledger run ID", &run_id)?;
        request.validate()?;
        system_instructions
            .validate()
            .map_err(|error| ExecutionLedgerError::InvalidRecord(error.to_string()))?;
        next_action.validate()?;
        Ok(Self {
            run_id,
            request,
            system_instructions,
            sequence: 0,
            workspaces: BTreeMap::new(),
            observations: Vec::new(),
            permissions: Vec::new(),
            mutations: Vec::new(),
            unresolved_failures: Vec::new(),
            partial_outcomes: Vec::new(),
            budgets,
            next_action,
        })
    }

    /// Restores a persisted ledger only after validating all facts and bounds.
    ///
    /// # Errors
    ///
    /// Returns an error for unsupported versions, malformed provenance,
    /// duplicate IDs, invalid capability scopes, or oversized state.
    pub fn restore(snapshot: ExecutionLedgerSnapshot) -> Result<Self, ExecutionLedgerError> {
        validate_snapshot(&snapshot)?;
        Ok(Self {
            run_id: snapshot.run_id,
            request: snapshot.request,
            system_instructions: snapshot.system_instructions,
            sequence: snapshot.sequence,
            workspaces: snapshot.workspaces,
            observations: snapshot.observations,
            permissions: snapshot.permissions,
            mutations: snapshot.mutations,
            unresolved_failures: snapshot.unresolved_failures,
            partial_outcomes: snapshot.partial_outcomes,
            budgets: snapshot.budgets,
            next_action: snapshot.next_action,
        })
    }

    /// Returns a validated persistable snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error when serialized state exceeds the ledger size limit.
    pub fn snapshot(&self) -> Result<ExecutionLedgerSnapshot, ExecutionLedgerError> {
        let snapshot = ExecutionLedgerSnapshot {
            version: LEDGER_VERSION,
            run_id: self.run_id.clone(),
            request: self.request.clone(),
            system_instructions: self.system_instructions.clone(),
            sequence: self.sequence,
            workspaces: self.workspaces.clone(),
            observations: self.observations.clone(),
            permissions: self.permissions.clone(),
            mutations: self.mutations.clone(),
            unresolved_failures: self.unresolved_failures.clone(),
            partial_outcomes: self.partial_outcomes.clone(),
            budgets: self.budgets,
            next_action: self.next_action.clone(),
        };
        ensure_snapshot_size(&snapshot)?;
        Ok(snapshot)
    }

    /// Returns the immutable current request scope.
    #[must_use]
    pub const fn request(&self) -> &LedgerRequest {
        &self.request
    }

    /// Returns the exact trusted system-instruction identity for this run.
    #[must_use]
    pub const fn system_instructions(&self) -> &AgentSystemInstructionIdentity {
        &self.system_instructions
    }

    /// Returns remaining trusted budgets.
    #[must_use]
    pub const fn budgets(&self) -> ExecutionBudgets {
        self.budgets
    }

    /// Returns the required next action or terminal state.
    #[must_use]
    pub const fn next_action(&self) -> &LedgerNextAction {
        &self.next_action
    }

    /// Returns whether at least one successful mutation is authoritative.
    #[must_use]
    pub fn has_authoritative_mutation(&self) -> bool {
        !self.mutations.is_empty()
    }

    /// Returns whether a recorded failure still requires follow-up.
    #[must_use]
    pub fn has_unresolved_failures(&self) -> bool {
        !self.unresolved_failures.is_empty()
    }

    /// Returns whether a partial outcome has been recorded.
    #[must_use]
    pub fn has_partial_outcomes(&self) -> bool {
        !self.partial_outcomes.is_empty()
    }

    /// Adds or refreshes one host-resolved workspace alias.
    ///
    /// Existing facts remain immutable; changing the capability revision makes
    /// earlier observations ineligible for reuse.
    ///
    /// # Errors
    ///
    /// Returns an error when the workspace bound is reached.
    pub fn resolve_workspace(
        &mut self,
        workspace: WorkspaceResolution,
    ) -> Result<(), ExecutionLedgerError> {
        workspace.validate()?;
        if !self.workspaces.contains_key(workspace.alias())
            && self.workspaces.len() >= MAX_WORKSPACES
        {
            return Err(ExecutionLedgerError::LimitExceeded);
        }
        self.workspaces
            .insert(workspace.alias().to_owned(), workspace);
        Ok(())
    }

    /// Replaces trusted remaining budgets.
    pub const fn set_budgets(&mut self, budgets: ExecutionBudgets) {
        self.budgets = budgets;
    }

    /// Replaces the host-required next action.
    ///
    /// # Errors
    ///
    /// Returns an error for a malformed call-bound action.
    pub fn set_next_action(
        &mut self,
        action: LedgerNextAction,
    ) -> Result<(), ExecutionLedgerError> {
        action.validate()?;
        self.next_action = action;
        Ok(())
    }

    /// Records one authoritative observation from a completed exact tool result.
    ///
    /// # Errors
    ///
    /// Returns an error for mismatched provenance, unresolved aliases,
    /// malformed values, duplicate facts, or collection limits.
    pub fn record_observation(
        &mut self,
        request: &ValidatedToolRequest,
        result: &ToolResultEnvelope,
        result_id: &str,
        observation: NewObservation,
    ) -> Result<(), ExecutionLedgerError> {
        Self::require_result(request, result, true)?;
        validate_id("observation fact ID", &observation.fact_id)?;
        validate_id("observation result ID", result_id)?;
        validate_id("resource revision", &observation.resource_revision)?;
        validate_relative_path(&observation.relative_path)?;
        observation.values.validate()?;
        self.require_unique_id(&observation.fact_id)?;
        if self.observations.len() >= MAX_OBSERVATIONS {
            return Err(ExecutionLedgerError::LimitExceeded);
        }
        let workspace = self.workspace(&observation.workspace_alias)?.clone();
        let origin = self.origin(request, Some(result_id))?;
        let resource = LedgerResource::new(&workspace, observation.relative_path)?;
        let sequence = self.next_sequence()?;
        self.observations
            .push(ObservationFact::from_parts(FactParts {
                fact_id: observation.fact_id,
                sequence,
                origin,
                resource,
                resource_revision: observation.resource_revision,
                capability_revision: workspace.capability_revision().to_owned(),
                values: observation.values,
            }));
        Ok(())
    }

    /// Records an authoritative successful mutation from a completed write result.
    ///
    /// # Errors
    ///
    /// Returns an error unless the exact result completed a write or mutation,
    /// or when any changed resource/digest violates scope and bounds.
    pub fn record_mutation(
        &mut self,
        request: &ValidatedToolRequest,
        result: &ToolResultEnvelope,
        result_id: &str,
        mutation_id: impl Into<String>,
        changes: Vec<NewMutationChange>,
    ) -> Result<(), ExecutionLedgerError> {
        Self::require_result(request, result, true)?;
        if !matches!(
            request.definition().classification(),
            ToolClassification::Write | ToolClassification::Mutation
        ) {
            return Err(ExecutionLedgerError::WrongResultOutcome);
        }
        let mutation_id = mutation_id.into();
        validate_id("mutation ID", &mutation_id)?;
        validate_id("mutation result ID", result_id)?;
        self.require_unique_id(&mutation_id)?;
        if changes.is_empty()
            || changes.len() > MAX_MUTATION_CHANGES
            || self.mutations.len() >= MAX_MUTATIONS
        {
            return Err(ExecutionLedgerError::LimitExceeded);
        }
        let mut retained = Vec::with_capacity(changes.len());
        for change in changes {
            validate_relative_path(&change.relative_path)?;
            if let Some(previous) = &change.previous_sha256 {
                validate_digest(previous)?;
            }
            validate_digest(&change.resulting_sha256)?;
            validate_id("mutation resource revision", &change.resource_revision)?;
            let workspace = self.workspace(&change.workspace_alias)?;
            if workspace.capability() != LedgerCapability::ReadWrite {
                return Err(ExecutionLedgerError::ScopeMismatch);
            }
            retained.push(MutationChange::from_parts(MutationChangeParts {
                resource: LedgerResource::new(workspace, change.relative_path)?,
                previous_sha256: change.previous_sha256,
                resulting_sha256: change.resulting_sha256,
                resource_revision: change.resource_revision,
                capability_revision: workspace.capability_revision().to_owned(),
            }));
        }
        let origin = self.origin(request, Some(result_id))?;
        let sequence = self.next_sequence()?;
        self.mutations
            .push(MutationFact::new(mutation_id, sequence, origin, retained));
        Ok(())
    }

    /// Records a permission request or decision for one exact validated call.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed/duplicate IDs or collection limits.
    pub fn record_permission(
        &mut self,
        request: &ValidatedToolRequest,
        binding: &PermissionBinding,
        fact_id: impl Into<String>,
        permission_id: impl Into<String>,
        decision: LedgerPermissionDecision,
    ) -> Result<(), ExecutionLedgerError> {
        let fact_id = fact_id.into();
        let permission_id = permission_id.into();
        validate_id("permission fact ID", &fact_id)?;
        validate_id("permission ID", &permission_id)?;
        if !binding.matches_request_only(request)
            || binding.resource_scope().project_id() != self.request.project_id()
        {
            return Err(ExecutionLedgerError::ScopeMismatch);
        }
        self.require_unique_id(&fact_id)?;
        let existing = self
            .permissions
            .iter()
            .find(|fact| fact.permission_id() == permission_id);
        if existing.is_some_and(|fact| {
            fact.decision() != LedgerPermissionDecision::Pending
                || decision == LedgerPermissionDecision::Pending
        }) {
            return Err(ExecutionLedgerError::DuplicateFact(permission_id));
        }
        if self.permissions.len() >= MAX_PERMISSIONS {
            return Err(ExecutionLedgerError::LimitExceeded);
        }
        let origin = self.origin(request, None)?;
        let sequence = self.next_sequence()?;
        self.permissions.push(PermissionFact::new(
            fact_id,
            permission_id,
            sequence,
            origin,
            binding.clone(),
            decision,
        ));
        Ok(())
    }

    /// Records one unresolved failure from the exact failed result.
    ///
    /// # Errors
    ///
    /// Returns an error for a mismatched/non-failed result, malformed details,
    /// duplicate IDs, or collection limits.
    pub fn record_unresolved_failure(
        &mut self,
        request: &ValidatedToolRequest,
        result: &ToolResultEnvelope,
        result_id: &str,
        failure_id: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Result<(), ExecutionLedgerError> {
        Self::require_result(request, result, false)?;
        let failure_id = failure_id.into();
        let code = code.into();
        let message = message.into();
        validate_id("failure ID", &failure_id)?;
        validate_id("failure result ID", result_id)?;
        validate_id("failure code", &code)?;
        validate_message("failure message", &message)?;
        self.require_unique_id(&failure_id)?;
        if self.unresolved_failures.len() >= MAX_FAILURES {
            return Err(ExecutionLedgerError::LimitExceeded);
        }
        let origin = self.origin(request, Some(result_id))?;
        let sequence = self.next_sequence()?;
        self.unresolved_failures.push(UnresolvedFailure::new(
            failure_id, sequence, origin, code, message,
        ));
        Ok(())
    }

    /// Records a bounded partial outcome associated with one terminal result.
    ///
    /// # Errors
    ///
    /// Returns an error for mismatched provenance, malformed details,
    /// duplicate IDs, or collection limits.
    pub fn record_partial_outcome(
        &mut self,
        request: &ValidatedToolRequest,
        result: &ToolResultEnvelope,
        result_id: &str,
        outcome_id: impl Into<String>,
        summary: impl Into<String>,
    ) -> Result<(), ExecutionLedgerError> {
        Self::require_result(
            request,
            result,
            matches!(result.outcome(), ToolOutcome::Completed { .. }),
        )?;
        let outcome_id = outcome_id.into();
        let summary = summary.into();
        validate_id("partial outcome ID", &outcome_id)?;
        validate_id("partial result ID", result_id)?;
        validate_message("partial outcome summary", &summary)?;
        self.require_unique_id(&outcome_id)?;
        if self.partial_outcomes.len() >= MAX_PARTIAL_OUTCOMES {
            return Err(ExecutionLedgerError::LimitExceeded);
        }
        let origin = self.origin(request, Some(result_id))?;
        let sequence = self.next_sequence()?;
        self.partial_outcomes
            .push(PartialOutcome::new(outcome_id, sequence, origin, summary));
        Ok(())
    }

    /// Returns an observation only while every revision and capability
    /// assumption remains exact and no later mutation superseded the resource.
    #[must_use]
    pub fn reusable_observation(
        &self,
        fact_id: &str,
        context: &ReuseContext,
    ) -> Option<&ObservationFact> {
        let fact = self
            .observations
            .iter()
            .find(|candidate| candidate.fact_id() == fact_id)?;
        let workspace = self.workspaces.get(&context.workspace_alias)?;
        if context.project_id != self.request.project_id()
            || context.workspace_alias != fact.resource().workspace_alias()
            || context.capability_id != workspace.capability_id()
            || context.capability_id != fact.resource().capability_id()
            || context.capability_revision != workspace.capability_revision()
            || context.capability_revision != fact.capability_revision()
            || context.resource_revision != fact.resource_revision()
            || self.resource_mutated_after(fact)
        {
            return None;
        }
        Some(fact)
    }

    /// Returns the latest authoritative successful change for one resource.
    #[must_use]
    pub fn authoritative_mutation(
        &self,
        workspace_alias: &str,
        relative_path: &str,
    ) -> Option<&MutationChange> {
        self.mutations
            .iter()
            .rev()
            .flat_map(|mutation| mutation.changes().iter().rev())
            .find(|change| {
                change.resource().workspace_alias() == workspace_alias
                    && change.resource().relative_path() == relative_path
            })
    }

    /// Creates the bounded model-visible projection without authority data.
    #[must_use]
    pub fn model_projection(&self) -> ModelLedgerProjection {
        ModelLedgerProjection {
            request_id: self.request.request_id().to_owned(),
            project_id: self.request.project_id().to_owned(),
            current_request: self.request.text().to_owned(),
            workspaces: self
                .workspaces
                .values()
                .map(|workspace| ModelWorkspaceFact {
                    alias: workspace.alias().to_owned(),
                    capability: workspace.capability(),
                })
                .collect(),
            observations: self
                .observations
                .iter()
                .filter_map(|fact| {
                    Some(ModelObservationFact {
                        fact_id: fact.fact_id().to_owned(),
                        tool_call_id: fact.origin().tool_call_id().to_owned(),
                        result_id: fact.origin().result_id()?.to_owned(),
                        workspace_alias: fact.resource().workspace_alias().to_owned(),
                        relative_path: fact.resource().relative_path().to_owned(),
                        resource_revision: fact.resource_revision().to_owned(),
                        values: fact.values().clone(),
                    })
                })
                .collect(),
            permissions: self
                .permissions
                .iter()
                .map(|fact| ModelPermissionFact {
                    permission_id: fact.permission_id().to_owned(),
                    tool_call_id: fact.origin().tool_call_id().to_owned(),
                    tool_name: fact.tool_name().to_owned(),
                    decision: fact.decision(),
                })
                .collect(),
            mutations: self
                .mutations
                .iter()
                .filter_map(|fact| {
                    Some(ModelMutationFact {
                        mutation_id: fact.mutation_id().to_owned(),
                        tool_call_id: fact.origin().tool_call_id().to_owned(),
                        result_id: fact.origin().result_id()?.to_owned(),
                        changes: fact
                            .changes()
                            .iter()
                            .map(|change| ModelMutationChange {
                                workspace_alias: change.resource().workspace_alias().to_owned(),
                                relative_path: change.resource().relative_path().to_owned(),
                                resulting_sha256: change.resulting_sha256().to_owned(),
                                resource_revision: change.resource_revision().to_owned(),
                            })
                            .collect(),
                    })
                })
                .collect(),
            unresolved_failures: self
                .unresolved_failures
                .iter()
                .filter_map(|failure| {
                    Some(ModelFailureFact {
                        failure_id: failure.failure_id().to_owned(),
                        tool_call_id: failure.origin().tool_call_id().to_owned(),
                        result_id: failure.origin().result_id()?.to_owned(),
                        code: failure.code().to_owned(),
                        message: failure.message().to_owned(),
                    })
                })
                .collect(),
            partial_outcomes: self
                .partial_outcomes
                .iter()
                .filter_map(|outcome| {
                    Some(ModelPartialOutcome {
                        outcome_id: outcome.outcome_id().to_owned(),
                        tool_call_id: outcome.origin().tool_call_id().to_owned(),
                        result_id: outcome.origin().result_id()?.to_owned(),
                        summary: outcome.summary().to_owned(),
                    })
                })
                .collect(),
            budgets: self.budgets,
            next_action: self.next_action.clone(),
        }
    }

    fn workspace(&self, alias: &str) -> Result<&WorkspaceResolution, ExecutionLedgerError> {
        self.workspaces
            .get(alias)
            .ok_or_else(|| ExecutionLedgerError::UnknownWorkspace(alias.to_owned()))
    }

    fn origin(
        &self,
        request: &ValidatedToolRequest,
        result_id: Option<&str>,
    ) -> Result<FactOrigin, ExecutionLedgerError> {
        FactOrigin::new(
            self.request.request_id(),
            self.request.project_id(),
            request.call_id(),
            result_id,
        )
    }

    fn require_result(
        request: &ValidatedToolRequest,
        result: &ToolResultEnvelope,
        completed: bool,
    ) -> Result<(), ExecutionLedgerError> {
        if request.call_id() != result.call_id() || request.tool_name() != result.tool_name() {
            return Err(ExecutionLedgerError::ResultMismatch);
        }
        let outcome_matches = if completed {
            matches!(result.outcome(), ToolOutcome::Completed { .. })
        } else {
            matches!(result.outcome(), ToolOutcome::Failed { .. })
        };
        if !outcome_matches {
            return Err(ExecutionLedgerError::WrongResultOutcome);
        }
        Ok(())
    }

    fn next_sequence(&mut self) -> Result<u64, ExecutionLedgerError> {
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or(ExecutionLedgerError::LimitExceeded)?;
        Ok(self.sequence)
    }

    fn require_unique_id(&self, id: &str) -> Result<(), ExecutionLedgerError> {
        let duplicate = self.observations.iter().any(|fact| fact.fact_id() == id)
            || self.mutations.iter().any(|fact| fact.mutation_id() == id)
            || self.permissions.iter().any(|fact| fact.fact_id() == id)
            || self
                .unresolved_failures
                .iter()
                .any(|fact| fact.failure_id() == id)
            || self
                .partial_outcomes
                .iter()
                .any(|fact| fact.outcome_id() == id);
        if duplicate {
            return Err(ExecutionLedgerError::DuplicateFact(id.to_owned()));
        }
        Ok(())
    }

    fn resource_mutated_after(&self, observation: &ObservationFact) -> bool {
        self.mutations
            .iter()
            .filter(|mutation| mutation.sequence() > observation.sequence())
            .flat_map(MutationFact::changes)
            .any(|change| {
                change.resource().workspace_alias() == observation.resource().workspace_alias()
                    && change.resource().relative_path() == observation.resource().relative_path()
            })
    }
}

#[cfg(test)]
#[path = "ledger_tests.rs"]
mod tests;
