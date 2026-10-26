//! Focused plan-policy responsibility extracted from the feature coordinator.

//! Forecast evidence capabilities and postcondition verification.

use super::{
    ChatToolExchange, CompletedEvidence, EvidenceClass, EvidenceProvision, EvidenceRole,
    ForecastStep, IpcError, ObligationReceipt, ParsedToolCall, PlanActionClass,
    PlanContractVersion, PlanEvidenceClass, PlanEvidenceObligation, PlanEvidencePostcondition,
    PlannedOutcome, SemanticPlanContract, ToolOutcome, ToolResultPart, ToolStagePostcondition,
    find_paths_result_resolves_all_queries, policy_error,
};
use ipc_types::JsonValue;
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct AdmittedPlan {
    pub(super) contract_version: PlanContractVersion,
    pub(super) outcome: PlannedOutcome,
    pub(super) forecast: PlanForecast,
    pub(super) semantic_contract: Option<SemanticPlanContract>,
    pub(super) adaptive_allowance: usize,
    pub(super) outcome_binding_changed: bool,
    pub(super) signature: Option<String>,
}

pub(super) struct PlanForecast {
    pub(super) contract_version: PlanContractVersion,
    pub(super) estimated_calls: usize,
    pub(super) expected_tools: BTreeSet<String>,
    pub(super) steps: Vec<ForecastStep>,
    pub(super) diagnostics: Vec<String>,
}

impl PlanForecast {
    pub(super) fn has_action(&self, class: PlanActionClass) -> bool {
        self.steps
            .iter()
            .any(|step| step.action_class == Some(class))
    }

    pub(super) fn has_workspace_write_action(&self) -> bool {
        self.has_action(PlanActionClass::WorkspaceWrite)
    }

    pub(super) fn validate_proportional_evidence_route(&self) -> Result<(), IpcError> {
        for step in &self.steps {
            let unscoped_collector = step
                .calls
                .iter()
                .any(|call| call.tool == "filesystem.collect_evidence" && call.scope.is_none());
            let unscoped_legacy_read = step.calls.iter().any(|call| {
                call.scope.is_none()
                    && matches!(
                        call.tool.as_str(),
                        "filesystem.list"
                            | "filesystem.find_paths"
                            | "filesystem.stat"
                            | "filesystem.read_text"
                            | "filesystem.inspect_image"
                            | "filesystem.search"
                            | "filesystem.scan"
                    )
            });
            if unscoped_collector && unscoped_legacy_read {
                return Err(policy_error(
                    "One plan step forecasts both unscoped filesystem.collect_evidence and separate unscoped filesystem discovery or loading. Consolidate independent requirements into the collector or place genuinely dependent orientation in an earlier step",
                ));
            }
        }
        Ok(())
    }

    pub(super) fn has_typed_artifact_evidence(&self, completed: &[CompletedEvidence]) -> bool {
        let transform_step = self.steps.iter().position(|step| {
            step.calls
                .iter()
                .any(|call| call.tool == "orchestration.transform_artifact")
        });
        transform_step.is_some_and(|index| {
            let planned = self.steps[..index]
                .iter()
                .flat_map(|step| &step.calls)
                .flat_map(|call| &call.provides_evidence);
            let completed = completed.iter().map(|evidence| &evidence.provision);
            let provisions = planned.chain(completed).copied().collect::<Vec<_>>();
            provisions.iter().any(|provision| {
                provision.role == EvidenceRole::Source
                    && provision.class >= EvidenceClass::SemanticContent
            }) && provisions.iter().any(|provision| {
                provision.role == EvidenceRole::Convention
                    && provision.class >= EvidenceClass::FormatStructure
            })
        })
    }
}

pub(super) fn evidence_class_name(class: EvidenceClass) -> &'static str {
    match class {
        EvidenceClass::IdentityLocation => "identity_location",
        EvidenceClass::FormatStructure => "format_structure",
        EvidenceClass::SemanticContent => "semantic_content",
    }
}

pub(super) fn parse_evidence_provisions(
    expected_call: &serde_json::Map<String, serde_json::Value>,
    tool: &str,
    step_id: &str,
) -> Result<Vec<EvidenceProvision>, IpcError> {
    let Some(value) = expected_call.get("providesEvidence") else {
        return Ok(Vec::new());
    };
    let items = value.as_array().ok_or_else(|| {
        policy_error(&format!(
            "Plan step `{step_id}` expectedCalls for `{tool}` must encode providesEvidence as an array"
        ))
    })?;
    if items.len() > 4 {
        return Err(policy_error(&format!(
            "Plan step `{step_id}` expectedCalls for `{tool}` declares more than four evidence provisions"
        )));
    }
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let maximum = maximum_evidence_class(tool).ok_or_else(|| {
        policy_error(&format!(
            "Plan step `{step_id}` declares providesEvidence on `{tool}`, but that operation does not load or derive evidence"
        ))
    })?;
    let mut provisions = Vec::with_capacity(items.len());
    for item in items {
        let item = item.as_object().ok_or_else(|| {
            policy_error(&format!(
                "Plan step `{step_id}` providesEvidence entries for `{tool}` must be objects"
            ))
        })?;
        if item.keys().map(String::as_str).collect::<BTreeSet<_>>()
            != BTreeSet::from(["evidenceClass", "role"])
        {
            return Err(policy_error(&format!(
                "Plan step `{step_id}` providesEvidence entries for `{tool}` require exactly role and evidenceClass"
            )));
        }
        let role = match item.get("role").and_then(serde_json::Value::as_str) {
            Some("source") => EvidenceRole::Source,
            Some("convention") => EvidenceRole::Convention,
            _ => {
                return Err(policy_error(&format!(
                    "Plan step `{step_id}` providesEvidence for `{tool}` has an unknown role"
                )));
            }
        };
        let class = match item
            .get("evidenceClass")
            .and_then(serde_json::Value::as_str)
        {
            Some("identity_location") => EvidenceClass::IdentityLocation,
            Some("format_structure") => EvidenceClass::FormatStructure,
            Some("semantic_content") => EvidenceClass::SemanticContent,
            _ => {
                return Err(policy_error(&format!(
                    "Plan step `{step_id}` providesEvidence for `{tool}` has an unknown evidenceClass"
                )));
            }
        };
        if class > maximum {
            return Err(policy_error(&format!(
                "Plan step `{step_id}` cannot classify `{tool}` as {}; this operation provides at most {} evidence",
                evidence_class_name(class),
                evidence_class_name(maximum)
            )));
        }
        let provision = EvidenceProvision { role, class };
        if !provisions.contains(&provision) {
            provisions.push(provision);
        }
    }
    Ok(provisions)
}

pub(super) fn maximum_evidence_class(tool: &str) -> Option<EvidenceClass> {
    match tool {
        "filesystem.list" | "filesystem.find_paths" | "filesystem.stat" => {
            Some(EvidenceClass::IdentityLocation)
        }
        "filesystem.collect_evidence"
        | "filesystem.read_text"
        | "filesystem.inspect_image"
        | "filesystem.search"
        | "filesystem.scan"
        | "computation.run_project"
        | "web.retrieve"
        | "web.search" => Some(EvidenceClass::SemanticContent),
        _ => None,
    }
}

pub(super) fn maximum_semantic_evidence_class(call: &ParsedToolCall) -> Option<PlanEvidenceClass> {
    match call.name.as_str() {
        "filesystem.list" | "filesystem.find_paths" | "filesystem.stat" => {
            Some(PlanEvidenceClass::IdentityLocation)
        }
        "filesystem.collect_evidence" => call
            .arguments
            .get("items")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(
                |item| match item.get("operation").and_then(serde_json::Value::as_str) {
                    Some("find_matches") => Some(PlanEvidenceClass::IdentityLocation),
                    Some("load_exact_text_format" | "load_exact_image_format") => {
                        Some(PlanEvidenceClass::FormatStructure)
                    }
                    Some("load_exact_text_semantic" | "load_exact_image_semantic") => {
                        Some(PlanEvidenceClass::SemanticContent)
                    }
                    _ => None,
                },
            )
            .max(),
        "filesystem.read_text"
        | "filesystem.inspect_image"
        | "filesystem.search"
        | "filesystem.scan"
        | "web.retrieve"
        | "web.search" => Some(PlanEvidenceClass::SemanticContent),
        _ => None,
    }
}

pub(super) fn obligation_is_step_output(
    action_class: Option<PlanActionClass>,
    obligation: &PlanEvidenceObligation,
) -> bool {
    match action_class {
        Some(PlanActionClass::Read) => true,
        Some(PlanActionClass::Compute) => {
            obligation.postcondition == PlanEvidencePostcondition::ComputationCompleted
        }
        Some(PlanActionClass::WorkspaceWrite) => matches!(
            obligation.postcondition,
            PlanEvidencePostcondition::FileWriteVerified
                | PlanEvidencePostcondition::DirectoryCreateVerified
        ),
        Some(PlanActionClass::ExternalWrite) => {
            obligation.postcondition == PlanEvidencePostcondition::ExternalEffectVerified
        }
        None => false,
    }
}

pub(super) fn operation_can_address_obligations(
    call: &ParsedToolCall,
    obligations: &[&PlanEvidenceObligation],
) -> bool {
    if obligations.is_empty() {
        return true;
    }
    if call.name == "filesystem.collect_evidence" {
        return obligations
            .iter()
            .all(|obligation| collector_item_for_obligation(call, obligation).is_some());
    }
    obligations.len() == 1 && operation_supports_obligation(call, obligations[0])
}

pub(super) fn operation_name_can_address_obligations(
    operation: &str,
    obligations: &[&PlanEvidenceObligation],
) -> bool {
    if operation == "filesystem.collect_evidence" {
        return true;
    }
    operation_can_address_obligations(
        &ParsedToolCall {
            call_id: String::new(),
            recipient: operation.to_owned(),
            name: operation.to_owned(),
            arguments: BTreeMap::new(),
        },
        obligations,
    )
}

pub(super) fn operation_supports_obligation(
    call: &ParsedToolCall,
    obligation: &PlanEvidenceObligation,
) -> bool {
    let (maximum, postconditions): (_, &[PlanEvidencePostcondition]) = match call.name.as_str() {
        "filesystem.list" | "filesystem.find_paths" | "filesystem.stat" => (
            PlanEvidenceClass::IdentityLocation,
            &[PlanEvidencePostcondition::IdentityLocated],
        ),
        "filesystem.read_text"
        | "filesystem.search"
        | "filesystem.scan"
        | "web.retrieve"
        | "web.search" => (
            PlanEvidenceClass::SemanticContent,
            &[
                PlanEvidencePostcondition::FormatLoaded,
                PlanEvidencePostcondition::TextFormatLoaded,
                PlanEvidencePostcondition::ContentLoaded,
                PlanEvidencePostcondition::TextContentLoaded,
            ],
        ),
        "filesystem.inspect_image" => (
            PlanEvidenceClass::SemanticContent,
            &[
                PlanEvidencePostcondition::FormatLoaded,
                PlanEvidencePostcondition::ImageFormatLoaded,
                PlanEvidencePostcondition::ContentLoaded,
                PlanEvidencePostcondition::ImageContentLoaded,
            ],
        ),
        "computation.run" | "computation.run_project" => (
            PlanEvidenceClass::SemanticContent,
            &[PlanEvidencePostcondition::ComputationCompleted],
        ),
        "filesystem.write_text"
        | "filesystem.replace_text"
        | "filesystem.write_delimited_table"
        | "orchestration.transform_artifact" => (
            PlanEvidenceClass::IdentityLocation,
            &[PlanEvidencePostcondition::FileWriteVerified],
        ),
        "filesystem.create_directory" => (
            PlanEvidenceClass::IdentityLocation,
            &[PlanEvidencePostcondition::DirectoryCreateVerified],
        ),
        _ => return false,
    };
    maximum >= obligation.evidence_class && postconditions.contains(&obligation.postcondition)
}

pub(super) fn collector_item_for_obligation<'a>(
    call: &'a ParsedToolCall,
    obligation: &PlanEvidenceObligation,
) -> Option<&'a serde_json::Value> {
    call.arguments
        .get("items")?
        .as_array()?
        .iter()
        .find(|item| {
            item.get("id").and_then(serde_json::Value::as_str) == Some(obligation.id.as_str())
                && collector_operation_capability(item).is_some_and(|(class, postconditions)| {
                    class >= obligation.evidence_class
                        && postconditions.contains(&obligation.postcondition)
                })
        })
}

pub(super) fn collector_operation_capability(
    item: &serde_json::Value,
) -> Option<(PlanEvidenceClass, &'static [PlanEvidencePostcondition])> {
    collector_operation_name_capability(item.get("operation")?.as_str()?)
}

pub(super) fn collector_operations_for_obligation(
    obligation: &PlanEvidenceObligation,
) -> Vec<&'static str> {
    [
        "find_matches",
        "load_exact_text_format",
        "load_exact_image_format",
        "load_exact_text_semantic",
        "load_exact_image_semantic",
    ]
    .into_iter()
    .filter(|operation| {
        collector_operation_name_capability(operation).is_some_and(|(class, postconditions)| {
            class >= obligation.evidence_class && postconditions.contains(&obligation.postcondition)
        })
    })
    .collect()
}

pub(super) fn collector_operation_name_capability(
    operation: &str,
) -> Option<(PlanEvidenceClass, &'static [PlanEvidencePostcondition])> {
    match operation {
        "find_matches" => Some((
            PlanEvidenceClass::IdentityLocation,
            &[PlanEvidencePostcondition::IdentityLocated],
        )),
        "load_exact_text_format" => Some((
            PlanEvidenceClass::FormatStructure,
            &[
                PlanEvidencePostcondition::FormatLoaded,
                PlanEvidencePostcondition::TextFormatLoaded,
            ],
        )),
        "load_exact_image_format" => Some((
            PlanEvidenceClass::FormatStructure,
            &[
                PlanEvidencePostcondition::FormatLoaded,
                PlanEvidencePostcondition::ImageFormatLoaded,
            ],
        )),
        "load_exact_text_semantic" => Some((
            PlanEvidenceClass::SemanticContent,
            &[
                PlanEvidencePostcondition::FormatLoaded,
                PlanEvidencePostcondition::TextFormatLoaded,
                PlanEvidencePostcondition::ContentLoaded,
                PlanEvidencePostcondition::TextContentLoaded,
            ],
        )),
        "load_exact_image_semantic" => Some((
            PlanEvidenceClass::SemanticContent,
            &[
                PlanEvidencePostcondition::FormatLoaded,
                PlanEvidencePostcondition::ImageFormatLoaded,
                PlanEvidencePostcondition::ContentLoaded,
                PlanEvidencePostcondition::ImageContentLoaded,
            ],
        )),
        _ => None,
    }
}

pub(super) fn verified_obligation_receipts(
    call: &ParsedToolCall,
    exchange: &ChatToolExchange,
    obligations: &[&PlanEvidenceObligation],
) -> Option<Vec<(String, ObligationReceipt)>> {
    if call.name == "filesystem.collect_evidence" {
        return verified_collector_obligation_receipts(call, exchange, obligations);
    }
    let [obligation] = obligations else {
        return None;
    };
    if !operation_supports_obligation(call, obligation)
        || !exchange_proves_obligation(call, exchange, obligation)
    {
        return None;
    }
    Some(vec![(
        obligation.id.clone(),
        ObligationReceipt {
            call_id: call.call_id.clone(),
            output_key: "single".to_owned(),
            role_kind: obligation.role_kind,
            evidence_class: obligation.evidence_class,
            postcondition: obligation.postcondition,
        },
    )])
}

pub(super) fn verified_collector_obligation_receipts(
    call: &ParsedToolCall,
    exchange: &ChatToolExchange,
    obligations: &[&PlanEvidenceObligation],
) -> Option<Vec<(String, ObligationReceipt)>> {
    let ToolOutcome::Completed { content } = &exchange.result.outcome else {
        return None;
    };
    let root = content.iter().find_map(|part| {
        let ToolResultPart::Json {
            value: JsonValue::Object(root),
        } = part
        else {
            return None;
        };
        (root.get("ready") == Some(&JsonValue::Bool(true))
            && root.get("coverageComplete") == Some(&JsonValue::Bool(true)))
        .then_some(root)
    })?;
    let JsonValue::Array(items) = root.get("items")? else {
        return None;
    };
    obligations
        .iter()
        .map(|obligation| {
            collector_item_for_obligation(call, obligation)?;
            let item = items.iter().find_map(|item| {
                let JsonValue::Object(fields) = item else {
                    return None;
                };
                (fields.get("id") == Some(&JsonValue::String(obligation.id.clone()))
                    && fields.get("status") == Some(&JsonValue::String("ready".to_owned())))
                .then_some(fields)
            })?;
            if !collector_result_item_proves(item, exchange, obligation.postcondition) {
                return None;
            }
            Some((
                obligation.id.clone(),
                ObligationReceipt {
                    call_id: call.call_id.clone(),
                    output_key: format!("item:{}", obligation.id),
                    role_kind: obligation.role_kind,
                    evidence_class: obligation.evidence_class,
                    postcondition: obligation.postcondition,
                },
            ))
        })
        .collect()
}

pub(super) fn collector_result_item_proves(
    fields: &BTreeMap<String, JsonValue>,
    exchange: &ChatToolExchange,
    postcondition: PlanEvidencePostcondition,
) -> bool {
    match postcondition {
        PlanEvidencePostcondition::IdentityLocated => fields.contains_key("discovery"),
        PlanEvidencePostcondition::FormatLoaded | PlanEvidencePostcondition::ContentLoaded => {
            fields.contains_key("text")
                || (fields.contains_key("image") && exchange.inspected_asset_id.is_some())
        }
        PlanEvidencePostcondition::TextFormatLoaded
        | PlanEvidencePostcondition::TextContentLoaded => fields.contains_key("text"),
        PlanEvidencePostcondition::ImageFormatLoaded
        | PlanEvidencePostcondition::ImageContentLoaded => {
            fields.contains_key("image") && exchange.inspected_asset_id.is_some()
        }
        _ => false,
    }
}

pub(super) fn exchange_proves_obligation(
    call: &ParsedToolCall,
    exchange: &ChatToolExchange,
    obligation: &PlanEvidenceObligation,
) -> bool {
    let ToolOutcome::Completed { content } = &exchange.result.outcome else {
        return false;
    };
    match obligation.postcondition {
        PlanEvidencePostcondition::IdentityLocated => {
            if call.name == "filesystem.find_paths" {
                find_paths_result_resolves_all_queries(call, exchange)
            } else {
                !content.is_empty()
            }
        }
        PlanEvidencePostcondition::TextFormatLoaded
        | PlanEvidencePostcondition::TextContentLoaded => {
            if call.name == "filesystem.read_text" {
                stage_postcondition_satisfied(ToolStagePostcondition::TextLoaded, exchange)
            } else {
                !content.is_empty()
            }
        }
        PlanEvidencePostcondition::ImageFormatLoaded
        | PlanEvidencePostcondition::ImageContentLoaded => exchange.inspected_asset_id.is_some(),
        PlanEvidencePostcondition::FormatLoaded | PlanEvidencePostcondition::ContentLoaded => {
            if call.name == "filesystem.inspect_image" {
                exchange.inspected_asset_id.is_some()
            } else if call.name == "filesystem.read_text" {
                stage_postcondition_satisfied(ToolStagePostcondition::TextLoaded, exchange)
            } else {
                !content.is_empty()
            }
        }
        PlanEvidencePostcondition::ComputationCompleted => {
            stage_postcondition_satisfied(ToolStagePostcondition::ComputationCompleted, exchange)
        }
        PlanEvidencePostcondition::FileWriteVerified => {
            stage_postcondition_satisfied(ToolStagePostcondition::FileWriteVerified, exchange)
        }
        PlanEvidencePostcondition::DirectoryCreateVerified => {
            stage_postcondition_satisfied(ToolStagePostcondition::DirectoryCreateVerified, exchange)
        }
        PlanEvidencePostcondition::ExternalEffectVerified => false,
    }
}

pub(super) fn postcondition_satisfies(
    provided: PlanEvidencePostcondition,
    required: PlanEvidencePostcondition,
) -> bool {
    provided == required
        || matches!(
            (provided, required),
            (
                PlanEvidencePostcondition::TextFormatLoaded
                    | PlanEvidencePostcondition::ImageFormatLoaded,
                PlanEvidencePostcondition::FormatLoaded
            ) | (
                PlanEvidencePostcondition::TextContentLoaded
                    | PlanEvidencePostcondition::ImageContentLoaded,
                PlanEvidencePostcondition::ContentLoaded
            )
        )
}

pub(super) fn is_evidence_read_tool(name: &str) -> bool {
    matches!(
        name,
        "filesystem.list"
            | "filesystem.find_paths"
            | "filesystem.collect_evidence"
            | "filesystem.stat"
            | "filesystem.read_text"
            | "filesystem.inspect_image"
            | "filesystem.search"
            | "filesystem.scan"
            | "web.retrieve"
            | "web.search"
    )
}

pub(super) fn is_safe_read_tool(name: &str) -> bool {
    is_evidence_read_tool(name) || matches!(name, "computation.run" | "computation.run_project")
}

pub(super) fn dependency_output_evidence_classes(
    step: &ForecastStep,
) -> BTreeSet<PlanEvidenceClass> {
    let mut classes = step
        .semantic_required_evidence
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    if step.action_class == Some(PlanActionClass::Compute) {
        classes.insert(PlanEvidenceClass::FormatStructure);
        classes.insert(PlanEvidenceClass::SemanticContent);
    }
    if step.action_class == Some(PlanActionClass::WorkspaceWrite) {
        classes.insert(PlanEvidenceClass::IdentityLocation);
    }
    classes
}

pub(super) fn stage_postcondition(operation: &str) -> Option<ToolStagePostcondition> {
    match operation {
        "filesystem.read_text" => Some(ToolStagePostcondition::TextLoaded),
        "filesystem.inspect_image" => Some(ToolStagePostcondition::ImageLoaded),
        "computation.run" | "computation.run_project" => {
            Some(ToolStagePostcondition::ComputationCompleted)
        }
        "filesystem.write_text"
        | "filesystem.replace_text"
        | "filesystem.write_delimited_table"
        | "orchestration.transform_artifact" => Some(ToolStagePostcondition::FileWriteVerified),
        "filesystem.create_directory" => Some(ToolStagePostcondition::DirectoryCreateVerified),
        _ => None,
    }
}

pub(super) fn stage_postcondition_satisfied(
    postcondition: ToolStagePostcondition,
    exchange: &ChatToolExchange,
) -> bool {
    let ToolOutcome::Completed { content } = &exchange.result.outcome else {
        return false;
    };
    match postcondition {
        ToolStagePostcondition::TextLoaded => content.iter().any(|part| {
            matches!(part, ToolResultPart::Json { value: JsonValue::Object(fields) }
                if matches!(fields.get("text"), Some(JsonValue::String(_))))
        }),
        ToolStagePostcondition::ImageLoaded => exchange.inspected_asset_id.is_some(),
        ToolStagePostcondition::ComputationCompleted => {
            exchange.computation_reuse_digest.is_some() && !content.is_empty()
        }
        ToolStagePostcondition::FileWriteVerified => content.iter().any(|part| {
            let ToolResultPart::Json {
                value: JsonValue::Object(fields),
            } = part
            else {
                return false;
            };
            let Some(JsonValue::Array(changes)) = fields.get("changes") else {
                return false;
            };
            matches!(
                fields.get("postconditionVerified"),
                Some(JsonValue::Bool(true))
            ) && !changes.is_empty()
                && changes.iter().all(|change| {
                    matches!(change, JsonValue::Object(fields)
                        if matches!(fields.get("postWriteVerified"), Some(JsonValue::Bool(true))))
                })
        }),
        ToolStagePostcondition::DirectoryCreateVerified => content.iter().any(|part| {
            matches!(part, ToolResultPart::Json { value: JsonValue::Object(fields) }
                if matches!(fields.get("postCreateVerified"), Some(JsonValue::Bool(true))))
        }),
    }
}
