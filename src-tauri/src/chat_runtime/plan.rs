//! Host-validated, model-authored provisional workspace plans.

mod admission;
mod contract;
mod direct_transfer;
pub(super) use direct_transfer::is_standalone_transfer;
mod forecast;
mod parsing;
mod projection;
mod results;
mod routing;

use forecast::{
    AdmittedPlan, PlanForecast, collector_operations_for_obligation,
    dependency_output_evidence_classes, evidence_class_name, is_evidence_read_tool,
    is_safe_read_tool, maximum_semantic_evidence_class, obligation_is_step_output,
    operation_can_address_obligations, operation_name_can_address_obligations,
    parse_evidence_provisions, postcondition_satisfies, stage_postcondition,
    stage_postcondition_satisfied, verified_obligation_receipts,
};
use parsing::{
    ExecutionForecast, invalid_arguments, parse_execution_forecast, parse_semantic_steps,
    parse_steps, plan_contract_metadata, policy_error, string,
};
use projection::{
    exchange_satisfies_forecast, plan_signature, project_collector_item_schema,
    project_forecast_mode, project_plan_mode, retain_recent_unique, retain_semantic_progress,
    validate_forecast_outcome,
};

use std::collections::{BTreeMap, BTreeSet};

use app_core::{
    ToolBoundedStageContract, ToolPlanOutcome, ToolPlanProgress, ToolPlanStepProgress,
    ToolPlanStepStatus, ToolStagePostcondition,
};
use ipc_types::{
    IpcError, IpcErrorCode, TaskActivityStatus, ToolName, ToolOutcome, ToolResultPart,
};
use model_runtime::{ParsedToolCall, PromptToolDefinition};

use crate::chat_tools::{
    ChatToolExchange, EXECUTION_FORECAST_CONTRACT_VERSION,
    LEGACY_EXECUTION_FORECAST_CONTRACT_VERSION, MAX_PROVISIONAL_PLAN_STEPS,
    OperationAdmissionRecord, OperationPermissionOutcome, OperationProgressContribution,
    OperationStepRelationship, ROLE_AWARE_EXECUTION_FORECAST_CONTRACT_VERSION, SEARCH_TOOLS_TOOL,
    ToolCategory, find_paths_result_resolves_all_queries, is_forecastable_chat_tool,
    project_tool_definitions, terminal_result_completes_stage, tool_category,
    tool_category_name as operation_category,
};

use contract::{
    PlanActionClass, PlanContractVersion, PlanEvidenceClass, PlanEvidenceObligation,
    PlanEvidencePostcondition, PlanEvidenceRoleKind, SemanticPlanContract,
    plan_repair_defects as contract_repair_defects,
};

pub(super) fn plan_repair_defects(error: &IpcError) -> Option<Vec<(String, String)>> {
    contract_repair_defects(error)
}

const PLAN_TOOL: &str = "orchestration.submit_plan";
pub(super) const FORECAST_TOOL: &str = "orchestration.submit_forecast";
pub(super) const COMPLETE_STAGE_TOOL: &str = "orchestration.complete_stage";
const MAX_EXPECTED_CALLS_PER_STEP: usize = 8;
const MAX_PLAN_REVISIONS: usize = 3;
const MAX_CORRECTIONS_PER_REVISION: usize = 6;
const MAX_IDENTICAL_CORRECTIONS_PER_REVISION: usize = 3;
const MAX_EVIDENCE_REFINEMENTS_PER_REVISION: usize = 2;
const MAX_RETAINED_ADVISORY_RESULTS: usize = 8;
const PLAN_RESERVE_CALLS: usize = 2;
const BOUNDED_STAGE_RETRY_LIMIT: usize = 2;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct BoundedStageContract {
    operation: String,
    postcondition: ToolStagePostcondition,
    evidence_call_ids: Vec<String>,
}

impl BoundedStageContract {
    #[cfg(test)]
    pub(super) fn test_fixture(operation: &str, postcondition: ToolStagePostcondition) -> Self {
        Self {
            operation: operation.to_owned(),
            postcondition,
            evidence_call_ids: Vec::new(),
        }
    }

    pub(super) fn operation(&self) -> &str {
        &self.operation
    }

    pub(super) const fn retry_limit() -> usize {
        BOUNDED_STAGE_RETRY_LIMIT
    }

    pub(super) fn projection(&self) -> ToolBoundedStageContract {
        ToolBoundedStageContract::new(
            self.operation.clone(),
            self.postcondition,
            BOUNDED_STAGE_RETRY_LIMIT,
            self.evidence_call_ids.clone(),
        )
    }

    fn matches(&self, call: &ParsedToolCall) -> bool {
        self.operation == call.name
    }

    pub(super) fn postcondition_satisfied(
        &self,
        call: &ParsedToolCall,
        exchange: &Result<ChatToolExchange, IpcError>,
    ) -> bool {
        self.matches(call)
            && exchange
                .as_ref()
                .is_ok_and(|exchange| stage_postcondition_satisfied(self.postcondition, exchange))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ForecastDiagnosticMatch {
    Matched,
    Unforecasted,
    OutOfOrder,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PlannedOutcome {
    Answer,
    CreateFile,
    ModifyFile,
    Other,
}

impl From<PlannedOutcome> for ToolPlanOutcome {
    fn from(outcome: PlannedOutcome) -> Self {
        match outcome {
            PlannedOutcome::Answer => Self::Answer,
            PlannedOutcome::CreateFile => Self::CreateFile,
            PlannedOutcome::ModifyFile => Self::ModifyFile,
            PlannedOutcome::Other => Self::Other,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ForecastCall {
    tool: String,
    // Retained for purpose-aware diagnostics without reinjecting model-authored
    // prose into trusted runtime context.
    purpose: String,
    scope: Option<ForecastScope>,
    provides_evidence: Vec<EvidenceProvision>,
    completed: bool,
    completed_call_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EvidenceRole {
    Source,
    Convention,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum EvidenceClass {
    IdentityLocation,
    FormatStructure,
    SemanticContent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct EvidenceProvision {
    role: EvidenceRole,
    class: EvidenceClass,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CompletedEvidence {
    call_id: String,
    provision: EvidenceProvision,
}

#[derive(Clone, Debug, Default)]
struct CorrectionState {
    diagnostics: BTreeMap<String, usize>,
    total: usize,
    pending: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ForecastScope {
    argument: String,
    value: String,
}

impl ForecastCall {
    fn matches(&self, call: &ParsedToolCall) -> bool {
        self.tool == call.name
            && self.scope.as_ref().is_none_or(|scope| {
                call.arguments
                    .get(&scope.argument)
                    .is_some_and(|value| value_contains_forecast_scope(value, &scope.value))
            })
    }
}

fn value_contains_forecast_scope(value: &serde_json::Value, expected: &str) -> bool {
    match value {
        serde_json::Value::String(value) => value == expected,
        serde_json::Value::Array(values) => values
            .iter()
            .any(|value| value_contains_forecast_scope(value, expected)),
        serde_json::Value::Object(values) => values
            .values()
            .any(|value| value_contains_forecast_scope(value, expected)),
        _ => false,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ForecastStep {
    id: String,
    depends_on: Vec<String>,
    purpose: String,
    required_evidence: Vec<String>,
    completion_evidence: String,
    calls: Vec<ForecastCall>,
    action_class: Option<PlanActionClass>,
    semantic_required_evidence: Vec<PlanEvidenceClass>,
    semantic_obligations: Vec<PlanEvidenceObligation>,
    satisfied_obligations: BTreeMap<String, ObligationReceipt>,
    semantic_complete: bool,
    /// Explicit model checkpoint, distinct from a host-observed write effect.
    semantic_completion_declared: bool,
    semantic_completed_call_ids: Vec<String>,
    blocked: bool,
}

impl ForecastStep {
    fn complete(&self) -> bool {
        self.action_class.map_or_else(
            || !self.calls.is_empty() && self.calls.iter().all(|call| call.completed),
            |_| {
                if self.semantic_obligations.is_empty() {
                    self.semantic_complete
                } else {
                    self.semantic_obligations
                        .iter()
                        .all(|obligation| self.satisfied_obligations.contains_key(&obligation.id))
                }
            },
        )
    }

    fn unfinished(&self) -> bool {
        !self.complete()
    }

    fn has_completed_work(&self) -> bool {
        self.semantic_complete
            || !self.semantic_completed_call_ids.is_empty()
            || !self.satisfied_obligations.is_empty()
            || self.calls.iter().any(|call| call.completed)
    }

    fn matches_semantic_tool(&self, operation: &ParsedToolCall) -> bool {
        self.action_class
            .is_some_and(|class| class.matches_tool(&operation.name))
    }

    fn semantic_evidence_satisfied_by(&self, operation: &ParsedToolCall) -> bool {
        if !self.semantic_obligations.is_empty() {
            let outputs = self.output_obligations();
            return operation_can_address_obligations(operation, &outputs);
        }
        if self.action_class != Some(PlanActionClass::Read) {
            return true;
        }
        let required = self.semantic_required_evidence.iter().max().copied();
        required.is_none_or(|required| {
            maximum_semantic_evidence_class(operation).is_some_and(|provided| provided >= required)
        })
    }

    fn output_obligations(&self) -> Vec<&PlanEvidenceObligation> {
        self.semantic_obligations
            .iter()
            .filter(|obligation| obligation_is_step_output(self.action_class, obligation))
            .collect()
    }

    fn prerequisite_obligations(&self) -> Vec<&PlanEvidenceObligation> {
        self.semantic_obligations
            .iter()
            .filter(|obligation| !obligation_is_step_output(self.action_class, obligation))
            .collect()
    }

    fn matches_operation(&self, operation: &ParsedToolCall) -> bool {
        self.calls
            .iter()
            .any(|forecast| !forecast.completed && forecast.matches(operation))
            || self.matches_semantic_tool(operation)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ObligationReceipt {
    call_id: String,
    output_key: String,
    role_kind: PlanEvidenceRoleKind,
    evidence_class: PlanEvidenceClass,
    postcondition: PlanEvidencePostcondition,
}

#[derive(Clone, Default)]
pub(super) struct PlanPolicy {
    submitted: bool,
    revision: usize,
    revisions_used: usize,
    operations_used: usize,
    unforecasted_operations: usize,
    estimated_calls: usize,
    working_allowance: usize,
    outcome: Option<PlannedOutcome>,
    outcome_satisfied: bool,
    outcome_execution_reserve_available: bool,
    corrections: BTreeMap<usize, CorrectionState>,
    pending_deviation: Option<ForecastDiagnosticMatch>,
    forecast_steps: Vec<ForecastStep>,
    completed_evidence: Vec<CompletedEvidence>,
    completed_before_revision: usize,
    safe_read_tools: BTreeSet<String>,
    pending_safe_read_escapes: BTreeSet<String>,
    accepted_plan_signature: Option<String>,
    contract_version: Option<PlanContractVersion>,
    semantic_contract: Option<SemanticPlanContract>,
    execution_forecast: Option<ExecutionForecast>,
    forecast_revisions: usize,
    unresolved_evidence_call_ids: BTreeMap<String, String>,
    retained_advisory_call_ids: Vec<String>,
    loaded_task_tool_names: BTreeSet<String>,
    tool_surface_revision: usize,
    verified_outcome_call_ids: Vec<String>,
    evidence_refinements_used: usize,
    refinement_step_id: Option<String>,
}

#[cfg(test)]
mod tests;
