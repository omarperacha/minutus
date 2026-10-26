use serde::Serialize;

use crate::{
    ExecutionBudgets, LedgerCapability, LedgerNextAction, LedgerPermissionDecision, ObservedValues,
};

/// Authority-free workspace information visible to the model.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelWorkspaceFact {
    pub(crate) alias: String,
    pub(crate) capability: LedgerCapability,
}

/// Authority-free observed resource fact visible to the model.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelObservationFact {
    pub(crate) fact_id: String,
    pub(crate) tool_call_id: String,
    pub(crate) result_id: String,
    pub(crate) workspace_alias: String,
    pub(crate) relative_path: String,
    pub(crate) resource_revision: String,
    pub(crate) values: ObservedValues,
}

/// Authority-free successful mutation visible to the model.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelMutationFact {
    pub(crate) mutation_id: String,
    pub(crate) tool_call_id: String,
    pub(crate) result_id: String,
    pub(crate) changes: Vec<ModelMutationChange>,
}

/// One safe changed-resource projection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelMutationChange {
    pub(crate) workspace_alias: String,
    pub(crate) relative_path: String,
    pub(crate) resulting_sha256: String,
    pub(crate) resource_revision: String,
}

/// Authority-free permission fact visible to the model.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPermissionFact {
    pub(crate) permission_id: String,
    pub(crate) tool_call_id: String,
    pub(crate) tool_name: String,
    pub(crate) decision: LedgerPermissionDecision,
}

/// Sanitized unresolved failure visible to the model.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelFailureFact {
    pub(crate) failure_id: String,
    pub(crate) tool_call_id: String,
    pub(crate) result_id: String,
    pub(crate) code: String,
    pub(crate) message: String,
}

/// Sanitized partial outcome visible to the model.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPartialOutcome {
    pub(crate) outcome_id: String,
    pub(crate) tool_call_id: String,
    pub(crate) result_id: String,
    pub(crate) summary: String,
}

/// Bounded model-visible view of trusted execution facts.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelLedgerProjection {
    pub(crate) request_id: String,
    pub(crate) project_id: String,
    pub(crate) current_request: String,
    pub(crate) workspaces: Vec<ModelWorkspaceFact>,
    pub(crate) observations: Vec<ModelObservationFact>,
    pub(crate) permissions: Vec<ModelPermissionFact>,
    pub(crate) mutations: Vec<ModelMutationFact>,
    pub(crate) unresolved_failures: Vec<ModelFailureFact>,
    pub(crate) partial_outcomes: Vec<ModelPartialOutcome>,
    pub(crate) budgets: ExecutionBudgets,
    pub(crate) next_action: LedgerNextAction,
}
