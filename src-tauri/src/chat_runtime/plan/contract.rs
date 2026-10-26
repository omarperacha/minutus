//! Versioned semantic execution-plan contract.
//!
//! This module owns model-authored intent and progress metadata only. It does
//! not grant workspace capabilities, approve effects, or establish successful
//! tool outcomes; those remain host-owned runtime responsibilities.

use std::collections::{BTreeMap, BTreeSet};

use ipc_types::{IpcError, IpcErrorCode};
use model_runtime::ParsedToolCall;

use crate::chat_tools::{ToolActionClass, tool_action_class};

mod legacy;
mod repair;
mod v4;
mod v6;
mod validation;

use legacy::{order_satisfiable_dependencies, parse_step, valid_obligation_id};
use v4::parse_v4_contract;
use v6::parse_v6_contract;
use validation::{
    bounded_object_string, bounded_object_string_array, bounded_string, bounded_string_array,
    contract_error,
};

pub(super) fn plan_repair_defects(error: &IpcError) -> Option<Vec<(String, String)>> {
    repair::plan_repair_defects(error)
}

pub(super) use crate::chat_tools::SEMANTIC_PLAN_CONTRACT_VERSION;
use crate::chat_tools::{
    HOST_ROUTED_SEMANTIC_PLAN_CONTRACT_VERSION, LEGACY_SEMANTIC_PLAN_CONTRACT_VERSION,
    OUTCOME_SEMANTIC_PLAN_CONTRACT_VERSION, ROLE_AWARE_SEMANTIC_PLAN_CONTRACT_VERSION,
    ROLELESS_SEMANTIC_PLAN_CONTRACT_VERSION,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PlanContractVersion {
    /// Compatibility contract for plans accepted before EXEC-010.
    LegacyForecastV1,
    /// Semantic execution contract used for newly advertised plans.
    SemanticExecutionV1,
    /// Least-surprise semantic execution contract used from EXEC-016.
    SemanticExecutionV2,
    /// Role-aware semantic execution contract introduced by EXEC-018.
    SemanticExecutionV3,
    /// Tagged-capability semantic execution contract introduced after EXEC-020.
    SemanticExecutionV4,
    /// Host-routed tagged-capability contract introduced after forecast qualification failed.
    SemanticExecutionV5,
    /// Advisory staged-plan contract with promoted, non-binding tool suggestions.
    SemanticExecutionV6,
}

impl PlanContractVersion {
    pub(super) fn from_call(call: &ParsedToolCall) -> Result<Self, IpcError> {
        match call
            .arguments
            .get("contractVersion")
            .and_then(serde_json::Value::as_str)
        {
            None => Ok(Self::LegacyForecastV1),
            Some(LEGACY_SEMANTIC_PLAN_CONTRACT_VERSION) => Ok(Self::SemanticExecutionV1),
            Some(ROLELESS_SEMANTIC_PLAN_CONTRACT_VERSION) => Ok(Self::SemanticExecutionV2),
            Some(ROLE_AWARE_SEMANTIC_PLAN_CONTRACT_VERSION) => Ok(Self::SemanticExecutionV3),
            Some(OUTCOME_SEMANTIC_PLAN_CONTRACT_VERSION) => Ok(Self::SemanticExecutionV4),
            Some(HOST_ROUTED_SEMANTIC_PLAN_CONTRACT_VERSION) => Ok(Self::SemanticExecutionV5),
            Some(SEMANTIC_PLAN_CONTRACT_VERSION) => Ok(Self::SemanticExecutionV6),
            Some(_) => Err(contract_error(
                "contractVersion must name the advertised semantic execution contract",
            )),
        }
    }

    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::LegacyForecastV1 => "legacy_forecast_v1",
            Self::SemanticExecutionV1 => LEGACY_SEMANTIC_PLAN_CONTRACT_VERSION,
            Self::SemanticExecutionV2 => ROLELESS_SEMANTIC_PLAN_CONTRACT_VERSION,
            Self::SemanticExecutionV3 => ROLE_AWARE_SEMANTIC_PLAN_CONTRACT_VERSION,
            Self::SemanticExecutionV4 => OUTCOME_SEMANTIC_PLAN_CONTRACT_VERSION,
            Self::SemanticExecutionV5 => HOST_ROUTED_SEMANTIC_PLAN_CONTRACT_VERSION,
            Self::SemanticExecutionV6 => SEMANTIC_PLAN_CONTRACT_VERSION,
        }
    }

    pub(super) const fn is_semantic(self) -> bool {
        matches!(
            self,
            Self::SemanticExecutionV1
                | Self::SemanticExecutionV2
                | Self::SemanticExecutionV3
                | Self::SemanticExecutionV4
                | Self::SemanticExecutionV5
                | Self::SemanticExecutionV6
        )
    }

    pub(super) const fn requires_execution_forecast(self) -> bool {
        matches!(
            self,
            Self::SemanticExecutionV2 | Self::SemanticExecutionV3 | Self::SemanticExecutionV4
        )
    }

    pub(super) const fn uses_typed_obligations(self) -> bool {
        matches!(
            self,
            Self::SemanticExecutionV3 | Self::SemanticExecutionV4 | Self::SemanticExecutionV5
        )
    }

    pub(super) const fn uses_host_routing(self) -> bool {
        matches!(self, Self::SemanticExecutionV5)
    }

    pub(super) const fn uses_advisory_stages(self) -> bool {
        matches!(self, Self::SemanticExecutionV6)
    }

    pub(super) const fn requires_plan_only_round(self) -> bool {
        matches!(
            self,
            Self::SemanticExecutionV2
                | Self::SemanticExecutionV3
                | Self::SemanticExecutionV4
                | Self::SemanticExecutionV5
                | Self::SemanticExecutionV6
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum PlanActionClass {
    Read,
    Compute,
    WorkspaceWrite,
    ExternalWrite,
}

impl PlanActionClass {
    fn parse(
        value: &serde_json::Value,
        step_id: &str,
        contract_version: PlanContractVersion,
    ) -> Result<Self, IpcError> {
        let parsed = match (contract_version, value.as_str()) {
            (_, Some("read")) => Some(Self::Read),
            (PlanContractVersion::SemanticExecutionV1, Some("transform"))
            | (
                PlanContractVersion::SemanticExecutionV2
                | PlanContractVersion::SemanticExecutionV3
                | PlanContractVersion::SemanticExecutionV4
                | PlanContractVersion::SemanticExecutionV5
                | PlanContractVersion::SemanticExecutionV6,
                Some("compute"),
            ) => Some(Self::Compute),
            (PlanContractVersion::SemanticExecutionV1, Some("local_write"))
            | (
                PlanContractVersion::SemanticExecutionV2
                | PlanContractVersion::SemanticExecutionV3
                | PlanContractVersion::SemanticExecutionV4
                | PlanContractVersion::SemanticExecutionV5
                | PlanContractVersion::SemanticExecutionV6,
                Some("workspace_write"),
            ) => Some(Self::WorkspaceWrite),
            (_, Some("external_write")) => Some(Self::ExternalWrite),
            _ => None,
        };
        match parsed {
            Some(class) => Ok(class),
            _ => Err(contract_error(&format!(
                "Plan step `{step_id}` has an unknown actionClass"
            ))),
        }
    }

    pub(super) fn matches_tool(self, tool: &str) -> bool {
        let expected = match self {
            Self::Read => ToolActionClass::Read,
            Self::Compute => ToolActionClass::Compute,
            Self::WorkspaceWrite => ToolActionClass::WorkspaceWrite,
            Self::ExternalWrite => ToolActionClass::ExternalWrite,
        };
        tool_action_class(tool) == Some(expected)
    }

    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Compute => "compute",
            Self::WorkspaceWrite => "workspace_write",
            Self::ExternalWrite => "external_write",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum PlanEvidenceClass {
    IdentityLocation,
    FormatStructure,
    SemanticContent,
}

impl PlanEvidenceClass {
    fn parse(value: &serde_json::Value, step_id: &str) -> Result<Self, IpcError> {
        match value.as_str() {
            Some("identity_location") => Ok(Self::IdentityLocation),
            Some("format_structure") => Ok(Self::FormatStructure),
            Some("semantic_content") => Ok(Self::SemanticContent),
            _ => Err(contract_error(&format!(
                "Plan step `{step_id}` has an unknown requiredEvidence value"
            ))),
        }
    }

    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::IdentityLocation => "identity_location",
            Self::FormatStructure => "format_structure",
            Self::SemanticContent => "semantic_content",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum PlanEvidenceRoleKind {
    Source,
    Convention,
    Target,
    Input,
    Reference,
}

impl PlanEvidenceRoleKind {
    fn parse(value: &serde_json::Value, step_id: &str) -> Result<Self, IpcError> {
        match value.as_str() {
            Some("source") => Ok(Self::Source),
            Some("convention") => Ok(Self::Convention),
            Some("target") => Ok(Self::Target),
            Some("input") => Ok(Self::Input),
            Some("reference") => Ok(Self::Reference),
            _ => Err(contract_error(&format!(
                "Plan step `{step_id}` has an unknown obligation roleKind"
            ))),
        }
    }

    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Convention => "convention",
            Self::Target => "target",
            Self::Input => "input",
            Self::Reference => "reference",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum PlanEvidencePostcondition {
    IdentityLocated,
    FormatLoaded,
    TextFormatLoaded,
    ImageFormatLoaded,
    ContentLoaded,
    TextContentLoaded,
    ImageContentLoaded,
    ComputationCompleted,
    FileWriteVerified,
    DirectoryCreateVerified,
    ExternalEffectVerified,
}

impl PlanEvidencePostcondition {
    fn parse(value: &serde_json::Value, step_id: &str) -> Result<Self, IpcError> {
        match value.as_str() {
            Some("identity_located") => Ok(Self::IdentityLocated),
            Some("format_loaded") => Ok(Self::FormatLoaded),
            Some("text_format_loaded") => Ok(Self::TextFormatLoaded),
            Some("image_format_loaded") => Ok(Self::ImageFormatLoaded),
            Some("content_loaded") => Ok(Self::ContentLoaded),
            Some("text_content_loaded") => Ok(Self::TextContentLoaded),
            Some("image_content_loaded") => Ok(Self::ImageContentLoaded),
            Some("computation_completed") => Ok(Self::ComputationCompleted),
            Some("file_write_verified") => Ok(Self::FileWriteVerified),
            Some("directory_create_verified") => Ok(Self::DirectoryCreateVerified),
            Some("external_effect_verified") => Ok(Self::ExternalEffectVerified),
            _ => Err(contract_error(&format!(
                "Plan step `{step_id}` has an unknown obligation postcondition"
            ))),
        }
    }

    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::IdentityLocated => "identity_located",
            Self::FormatLoaded => "format_loaded",
            Self::TextFormatLoaded => "text_format_loaded",
            Self::ImageFormatLoaded => "image_format_loaded",
            Self::ContentLoaded => "content_loaded",
            Self::TextContentLoaded => "text_content_loaded",
            Self::ImageContentLoaded => "image_content_loaded",
            Self::ComputationCompleted => "computation_completed",
            Self::FileWriteVerified => "file_write_verified",
            Self::DirectoryCreateVerified => "directory_create_verified",
            Self::ExternalEffectVerified => "external_effect_verified",
        }
    }

    pub(super) const fn is_effect(self) -> bool {
        matches!(
            self,
            Self::ComputationCompleted
                | Self::FileWriteVerified
                | Self::DirectoryCreateVerified
                | Self::ExternalEffectVerified
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PlanEvidenceObligation {
    pub(super) id: String,
    pub(super) role_kind: PlanEvidenceRoleKind,
    pub(super) evidence_class: PlanEvidenceClass,
    pub(super) postcondition: PlanEvidencePostcondition,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SemanticPlanStep {
    pub(super) id: String,
    pub(super) objective: String,
    pub(super) depends_on: Vec<String>,
    pub(super) required_evidence: Vec<PlanEvidenceClass>,
    pub(super) obligations: Vec<PlanEvidenceObligation>,
    pub(super) done_when: String,
    pub(super) action_class: PlanActionClass,
    pub(super) preferred_tools: Vec<String>,
}

impl SemanticPlanStep {
    /// V6 IDs carry advisory checkpoint identity, not proof of equivalent prose.
    /// Changed work must use a new ID; structural changes always reopen a stage.
    pub(super) fn same_advisory_stage_as(&self, other: &Self) -> bool {
        self.id == other.id
            && self.action_class == other.action_class
            && self.depends_on.len() == other.depends_on.len()
            && self
                .depends_on
                .iter()
                .all(|id| other.depends_on.contains(id))
    }

    /// Route preferences may change without invalidating completed semantic work.
    /// Every other field belongs to the binding step contract.
    pub(super) fn same_work_as(&self, other: &Self) -> bool {
        self.id == other.id
            && self.objective == other.objective
            && self.depends_on == other.depends_on
            && self.required_evidence == other.required_evidence
            && self.obligations == other.obligations
            && self.done_when == other.done_when
            && self.action_class == other.action_class
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SemanticPlanContract {
    pub(super) target: String,
    pub(super) constraints: Vec<String>,
    pub(super) success_criteria: Vec<String>,
    pub(super) assumptions: Vec<String>,
    pub(super) steps: Vec<SemanticPlanStep>,
}

impl SemanticPlanContract {
    pub(super) fn same_binding_as(&self, other: &Self) -> bool {
        self.target == other.target
            && self.constraints == other.constraints
            && self.success_criteria == other.success_criteria
            && self.assumptions == other.assumptions
            && self.steps.len() == other.steps.len()
            && self.steps.iter().all(|step| {
                other
                    .steps
                    .iter()
                    .find(|candidate| candidate.id == step.id)
                    .is_some_and(|candidate| step.same_work_as(candidate))
            })
    }

    /// V6 assumptions describe provisional beliefs, while suggested tools are
    /// advisory. Rewording either after learning a fact does not change the
    /// executable plan. A material revision must change the target,
    /// constraints, success criteria, stage work or dependencies.
    pub(super) fn same_executable_plan_as(&self, other: &Self) -> bool {
        self.target == other.target
            && self.constraints == other.constraints
            && self.success_criteria == other.success_criteria
            && self.steps.len() == other.steps.len()
            && self.steps.iter().all(|step| {
                other
                    .steps
                    .iter()
                    .find(|candidate| candidate.id == step.id)
                    .is_some_and(|candidate| step.same_work_as(candidate))
            })
    }

    pub(super) fn parse(
        arguments: &BTreeMap<String, serde_json::Value>,
        available_tools: &BTreeSet<&str>,
        maximum_steps: usize,
        contract_version: PlanContractVersion,
    ) -> Result<Self, IpcError> {
        if matches!(
            contract_version,
            PlanContractVersion::SemanticExecutionV4 | PlanContractVersion::SemanticExecutionV5
        ) {
            return parse_v4_contract(arguments, maximum_steps, contract_version);
        }
        if contract_version == PlanContractVersion::SemanticExecutionV6 {
            return parse_v6_contract(arguments, available_tools, maximum_steps);
        }
        let target = arguments
            .get("target")
            .and_then(|value| bounded_string(value, 1, 512))
            .ok_or_else(|| contract_error("`target` must identify the requested deliverable"))?;
        let constraints = bounded_string_array(arguments, "constraints", 0, 8)?;
        let success_criteria = bounded_string_array(arguments, "successCriteria", 1, 8)?;
        let assumptions = bounded_string_array(arguments, "assumptions", 0, 8)?;
        let steps = arguments
            .get("steps")
            .and_then(serde_json::Value::as_array)
            .filter(|steps| !steps.is_empty() && steps.len() <= maximum_steps)
            .ok_or_else(|| contract_error("The semantic plan must contain one to sixteen steps"))?;
        let mut ids = BTreeSet::new();
        let mut obligation_ids = BTreeSet::new();
        let mut parsed = Vec::with_capacity(steps.len());
        for (index, step) in steps.iter().enumerate() {
            parsed.push(parse_step(
                index,
                step,
                &mut ids,
                &mut obligation_ids,
                available_tools,
                contract_version,
            )?);
        }
        let parsed = order_satisfiable_dependencies(parsed)?;
        Ok(Self {
            target,
            constraints,
            success_criteria,
            assumptions,
            steps: parsed,
        })
    }
}

#[cfg(test)]
mod tests;
