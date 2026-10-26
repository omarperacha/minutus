//! Focused plan-policy responsibility extracted from the feature coordinator.

//! Model-facing schema projection and retained plan signatures.

use super::{
    ChatToolExchange, CompletedEvidence, EvidenceClass, EvidenceProvision, FORECAST_TOOL,
    ForecastStep, IpcError, PLAN_TOOL, ParsedToolCall, PlanActionClass, PlanContractVersion,
    PlanForecast, PlannedOutcome, PromptToolDefinition, SemanticPlanContract, ToolOutcome,
    find_paths_result_resolves_all_queries, policy_error,
};
use std::collections::BTreeSet;

pub(super) fn project_collector_item_schema(
    item_schema: &mut serde_json::Value,
    obligation_ids: &BTreeSet<String>,
    operations: &BTreeSet<&str>,
) {
    let Some(properties) = item_schema
        .get_mut("properties")
        .and_then(serde_json::Value::as_object_mut)
    else {
        return;
    };
    if let Some(id_schema) = properties
        .get_mut("id")
        .and_then(serde_json::Value::as_object_mut)
    {
        id_schema.insert(
            "enum".to_owned(),
            serde_json::Value::Array(
                obligation_ids
                    .iter()
                    .cloned()
                    .map(serde_json::Value::String)
                    .collect(),
            ),
        );
        id_schema.insert(
            "description".to_owned(),
            serde_json::Value::String(
                "Exact dependency-ready obligation id supplied by the host.".to_owned(),
            ),
        );
    }
    if let Some(operation_schema) = properties
        .get_mut("operation")
        .and_then(serde_json::Value::as_object_mut)
    {
        let mut branch_operations = operation_schema
            .get("enum")
            .and_then(serde_json::Value::as_array)
            .map_or_else(
                || {
                    operations
                        .iter()
                        .map(|operation| serde_json::Value::String((*operation).to_owned()))
                        .collect()
                },
                |existing| {
                    existing
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .filter(|operation| operations.contains(operation))
                        .map(|operation| serde_json::Value::String(operation.to_owned()))
                        .collect::<Vec<_>>()
                },
            );
        branch_operations.sort_by(|left, right| left.as_str().cmp(&right.as_str()));
        operation_schema.insert(
            "enum".to_owned(),
            serde_json::Value::Array(branch_operations),
        );
        operation_schema.insert(
            "description".to_owned(),
            serde_json::Value::String(
                "Closed operations capable of satisfying a dependency-ready obligation. Pair each operation with the exact compatible obligation id."
                    .to_owned(),
            ),
        );
    }
}

pub(super) fn project_plan_mode(
    mut tool: PromptToolDefinition,
    mode: &str,
) -> PromptToolDefinition {
    if tool.name == PLAN_TOOL {
        if let Some(mode_enum) = tool.parameters.pointer_mut("/properties/mode/enum") {
            *mode_enum = serde_json::json!([mode]);
        }
        tool.description = if mode == "revise" {
            format!(
                "Optional recovery control. Call only when new evidence requires a material change to the target, constraints, success criteria, stages or dependencies. Learned facts, completed results and changed tool preferences belong in continued execution and do not require revision. If called, use mode=revise; otherwise execute a dependency-ready operation. {}",
                tool.description
            )
        } else {
            format!(
                "Current host state requires mode={mode}. {}",
                tool.description
            )
        };
    }
    tool
}

pub(super) fn project_forecast_mode(
    mut tool: PromptToolDefinition,
    mode: &str,
) -> PromptToolDefinition {
    if tool.name == FORECAST_TOOL {
        if let Some(mode_enum) = tool.parameters.pointer_mut("/properties/mode/enum") {
            *mode_enum = serde_json::json!([mode]);
        }
        tool.description = format!(
            "Current host state requires mode={mode}. {}",
            tool.description
        );
    }
    tool
}

pub(super) fn plan_signature(call: &ParsedToolCall) -> Option<String> {
    (call.name == PLAN_TOOL).then(|| {
        let mut arguments = call.arguments.clone();
        arguments.remove("mode");
        serde_json::to_string(&arguments).unwrap_or_default()
    })
}

pub(super) fn retain_recent_unique(
    retained: &mut Vec<String>,
    call_ids: impl IntoIterator<Item = String>,
    limit: usize,
) {
    for call_id in call_ids {
        if let Some(index) = retained.iter().position(|existing| *existing == call_id) {
            retained.remove(index);
        }
        retained.push(call_id);
        if retained.len() > limit {
            retained.remove(0);
        }
    }
}

pub(super) fn retain_semantic_progress(
    new_steps: &mut [ForecastStep],
    new_contract: Option<&SemanticPlanContract>,
    old_contract: Option<&SemanticPlanContract>,
    old_steps: &[ForecastStep],
    outcome_binding_changed: bool,
    preserve_advisory_observations: bool,
) -> usize {
    let (Some(new_contract), Some(old_contract)) = (new_contract, old_contract) else {
        return 0;
    };
    let mut retained_complete = 0;
    for new_step in new_steps {
        let Some(new_semantic) = new_contract
            .steps
            .iter()
            .find(|step| step.id == new_step.id)
        else {
            continue;
        };
        let Some(old_semantic) = old_contract
            .steps
            .iter()
            .find(|step| step.id == new_step.id)
        else {
            continue;
        };
        let Some(old_step) = old_steps.iter().find(|step| step.id == old_semantic.id) else {
            continue;
        };
        if preserve_advisory_observations {
            new_step.semantic_completed_call_ids = old_step.semantic_completed_call_ids.clone();
        }
        let effectful = matches!(
            new_semantic.action_class,
            PlanActionClass::WorkspaceWrite | PlanActionClass::ExternalWrite
        );
        let same_stage = if preserve_advisory_observations && !effectful {
            old_semantic.same_advisory_stage_as(new_semantic)
        } else {
            old_semantic.same_work_as(new_semantic)
        };
        if !same_stage {
            continue;
        }
        let effect_must_be_reverified = outcome_binding_changed && effectful;
        if effect_must_be_reverified {
            continue;
        }
        new_step.semantic_complete = old_step.semantic_complete;
        new_step.semantic_completion_declared = old_step.semantic_completion_declared;
        if !preserve_advisory_observations {
            new_step.semantic_completed_call_ids = old_step.semantic_completed_call_ids.clone();
        }
        new_step.satisfied_obligations = old_step.satisfied_obligations.clone();
        retained_complete += usize::from(new_step.complete());
    }
    retained_complete
}

pub(super) fn exchange_satisfies_forecast(
    call: &ParsedToolCall,
    exchange: &ChatToolExchange,
    required_provisions: &[EvidenceProvision],
) -> bool {
    let ToolOutcome::Completed { content } = &exchange.result.outcome else {
        return false;
    };
    if call.name != "filesystem.collect_evidence" {
        return call.name != "filesystem.find_paths"
            || find_paths_result_resolves_all_queries(call, exchange);
    }
    let collection_ready = content.iter().any(|part| {
        matches!(
            part,
            ipc_types::ToolResultPart::Json {
                value: ipc_types::JsonValue::Object(fields)
            }
                if fields.get("ready") == Some(&ipc_types::JsonValue::Bool(true))
                    && fields.get("coverageComplete")
                        == Some(&ipc_types::JsonValue::Bool(true))
        )
    });
    collection_ready
        && required_provisions
            .iter()
            .all(|provision| collector_supports_provision(call, *provision))
}

pub(super) fn collector_supports_provision(
    call: &ParsedToolCall,
    provision: EvidenceProvision,
) -> bool {
    call.arguments
        .get("items")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|item| {
                let Some(fields) = item.as_object() else {
                    return false;
                };
                let (class, loads_content) =
                    match fields.get("operation").and_then(serde_json::Value::as_str) {
                        Some("find_matches") => (EvidenceClass::IdentityLocation, false),
                        Some("load_exact_text_format" | "load_exact_image_format") => {
                            (EvidenceClass::FormatStructure, true)
                        }
                        Some("load_exact_text_semantic" | "load_exact_image_semantic") => {
                            (EvidenceClass::SemanticContent, true)
                        }
                        _ => return false,
                    };
                class >= provision.class
                    && (provision.class == EvidenceClass::IdentityLocation || loads_content)
            })
        })
}

pub(super) fn validate_forecast_outcome(
    forecast: &PlanForecast,
    outcome: PlannedOutcome,
    completed_evidence: &[CompletedEvidence],
) -> Result<(), IpcError> {
    if forecast.contract_version == PlanContractVersion::LegacyForecastV1
        && forecast
            .expected_tools
            .contains("orchestration.transform_artifact")
        && !forecast.has_typed_artifact_evidence(completed_evidence)
    {
        return Err(policy_error(
            "An artifact transformation plan must obtain typed evidence before transformation: source requires semantic_content and convention requires format_structure or stronger. Declare these roles in providesEvidence on compatible earlier calls, or bind evidence already completed under an accepted plan revision",
        ));
    }
    if forecast.contract_version.is_semantic() {
        let has_advertised_external_operation = forecast.contract_version
            == PlanContractVersion::SemanticExecutionV6
            && forecast
                .expected_tools
                .iter()
                .any(|tool| PlanActionClass::ExternalWrite.matches_tool(tool));
        if forecast.has_action(PlanActionClass::ExternalWrite) && !has_advertised_external_operation
        {
            return Err(policy_error(
                "A semantic plan declares actionClass `external_write`, but the advertised execution catalogue has no external-write implementation; the plan cannot hide expanded authority in metadata",
            ));
        }
        if outcome == PlannedOutcome::Answer && forecast.has_action(PlanActionClass::WorkspaceWrite)
        {
            return Err(policy_error(
                "The declared answer outcome is inconsistent with a workspace_write step. Declare create_file or modify_file when the requested deliverable is a workspace mutation",
            ));
        }
    }
    match outcome {
        PlannedOutcome::CreateFile
            if !forecast.has_workspace_write_action()
                && !forecast.expected_tools.contains("filesystem.write_text")
                && !forecast
                    .expected_tools
                    .contains("filesystem.write_delimited_table")
                && !forecast
                    .expected_tools
                    .contains("orchestration.transform_artifact") =>
        {
            Err(policy_error(
                "The declared create_file outcome has no outcome-bearing forecast. Add a workspace_write step using filesystem.write_text or filesystem.write_delimited_table",
            ))
        }
        PlannedOutcome::ModifyFile
            if !forecast.has_workspace_write_action()
                && !forecast.expected_tools.contains("filesystem.write_text")
                && !forecast.expected_tools.contains("filesystem.replace_text")
                && !forecast
                    .expected_tools
                    .contains("filesystem.write_delimited_table")
                && !forecast
                    .expected_tools
                    .contains("orchestration.transform_artifact") =>
        {
            Err(policy_error(
                "The declared modify_file outcome has no outcome-bearing forecast. Add a workspace_write step using filesystem.write_text, filesystem.replace_text, or filesystem.write_delimited_table",
            ))
        }
        _ => Ok(()),
    }
}
