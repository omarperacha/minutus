//! Model-visible tool catalogue, validation, execution, and persisted history.

mod admission;
mod artifact_transform;
mod catalog;
mod collect_evidence;
mod computation;
mod execution;
mod filesystem;
mod forecast_contract;
mod history;
mod operation_ledger;
mod orchestration;
pub(crate) use orchestration::{
    PLAN_PENDING_GUIDANCE, PLAN_REQUIRED_OPERATION, deferred_plan_exchanges, emit_plan_deferrals,
    plan_deferral_batch_is_bounded,
};
mod path_references;
mod plan_contract;
pub(crate) mod recall;
mod tool_search;
mod validation;
mod wallet_transfer;
mod web;
mod web_search;
mod x402;

/// Host default for an omitted model-facing recursive discovery depth.
const DEFAULT_FIND_PATH_DEPTH: usize = 8;
/// Maximum logical decomposition accepted in one provisional plan.
pub(crate) const MAX_PROVISIONAL_PLAN_STEPS: usize = 16;

use ipc_types::{
    AssetId, Conversation, FilesystemUndoChangeRecord, JsonValue, MessagePart, TaskActivity,
    ToolOutcome, ToolRequest, ToolResult, ToolResultPart, WorkspaceGrantId,
    WorkspacePermissionLevel,
};

#[derive(Clone)]
pub(crate) struct ChatToolWorkspace {
    pub(super) alias: String,
    pub(super) display_name: String,
    pub(super) grant_id: WorkspaceGrantId,
    pub(super) permission_level: WorkspacePermissionLevel,
}

#[derive(Clone)]
pub(crate) struct ChatToolExchange {
    pub(super) request: ToolRequest,
    pub(super) result: ToolResult,
    pub(super) activity: TaskActivity,
    pub(super) workspace_grant_id: Option<WorkspaceGrantId>,
    pub(super) undo_changes: Vec<FilesystemUndoChangeRecord>,
    pub(super) inspected_asset_id: Option<AssetId>,
    pub(super) computation_reuse_digest: Option<String>,
}

pub(super) struct ExecutedChatTool {
    pub(super) value: JsonValue,
    pub(super) undo_changes: Vec<FilesystemUndoChangeRecord>,
    pub(super) inspected_asset_id: Option<AssetId>,
}

pub(crate) use admission::{
    OperationAdmissionRecord, OperationPermissionOutcome, OperationProgressContribution,
    OperationStepRelationship, permission_outcome,
};
pub(crate) use artifact_transform::normalize_arguments as normalize_artifact_arguments;
pub(crate) use catalog::{
    ToolActionClass, ToolCategory, chat_tool_definitions, chat_tool_workspaces,
    project_tool_definitions, terminal_result_completes_stage, tool_action_class, tool_category,
    tool_category_index, tool_category_name, tool_companions, tool_has_bulk_arguments,
};
#[cfg(test)]
pub(crate) use filesystem::ipc_json_value;
pub(crate) use forecast_contract::{
    EXECUTION_FORECAST_CONTRACT_VERSION, LEGACY_EXECUTION_FORECAST_CONTRACT_VERSION,
    ROLE_AWARE_EXECUTION_FORECAST_CONTRACT_VERSION,
};
pub(crate) use history::persist_tool_exchanges;
pub(crate) use operation_ledger::OperationLedger;
pub(crate) use orchestration::{
    complete_control_call, execute_chat_tool_batch, inactive_tool_batch_repairable,
    invalid_tool_batch_repairable, mixed_read_only_invalid_batch_repairable,
    plan_prefixed_invalid_operation_batch_repairable, reject_final_response_tool_batch,
    reject_inactive_tool_batch, reject_invalid_tool_batch, reject_plan_deviation_batch,
    reject_unknown_tool_batch, unknown_tool_batch_repairable,
};
pub(crate) use path_references::PathReferenceLedger;
pub(crate) use plan_contract::{
    HOST_ROUTED_SEMANTIC_PLAN_CONTRACT_VERSION, LEGACY_SEMANTIC_PLAN_CONTRACT_VERSION,
    OUTCOME_SEMANTIC_PLAN_CONTRACT_VERSION, ROLE_AWARE_SEMANTIC_PLAN_CONTRACT_VERSION,
    ROLELESS_SEMANTIC_PLAN_CONTRACT_VERSION, SEMANTIC_PLAN_CONTRACT_VERSION,
};
pub(crate) use tool_search::{
    SEARCH_TOOLS_TOOL, search_tool_definitions, tool_definition as tool_search_definition,
};
pub(crate) use validation::{
    exact_argument_repair_error, is_forecastable_chat_tool, validated_chat_tool_request,
};
pub(crate) use x402::{active_task_references, discovery_tool_definition, is_discovery_call};

pub(crate) fn applied_paths(
    exchanges: &[Result<ChatToolExchange, ipc_types::IpcError>],
) -> Vec<String> {
    exchanges
        .iter()
        .filter_map(|exchange| exchange.as_ref().ok())
        .flat_map(|exchange| {
            let mut paths = exchange
                .undo_changes
                .iter()
                .map(|change| change.relative_path.clone())
                .collect::<Vec<_>>();
            if exchange.request.name == ipc_types::ToolName::FilesystemCreateDirectory
                && matches!(exchange.result.outcome, ToolOutcome::Completed { .. })
                && let Some(JsonValue::String(path)) = exchange.request.arguments.get("path")
            {
                paths.push(path.clone());
            }
            paths
        })
        .collect()
}

pub(crate) fn applied_changes_verified(
    exchanges: &[Result<ChatToolExchange, ipc_types::IpcError>],
) -> bool {
    exchanges
        .iter()
        .filter_map(|exchange| exchange.as_ref().ok())
        .filter(|exchange| {
            !exchange.undo_changes.is_empty()
                || exchange.request.name == ipc_types::ToolName::FilesystemCreateDirectory
        })
        .all(|exchange| {
            let ToolOutcome::Completed { content } = &exchange.result.outcome else {
                return false;
            };
            content.iter().any(|part| {
                let ToolResultPart::Json {
                    value: JsonValue::Object(fields),
                } = part
                else {
                    return false;
                };
                if exchange.request.name == ipc_types::ToolName::FilesystemCreateDirectory {
                    return matches!(
                        fields.get("postCreateVerified"),
                        Some(JsonValue::Bool(true))
                    );
                }
                let Some(JsonValue::Array(changes)) = fields.get("changes") else {
                    return false;
                };
                matches!(
                    fields.get("postconditionVerified"),
                    Some(JsonValue::Bool(true))
                ) && !changes.is_empty()
                    && changes.iter().all(|change| {
                        let JsonValue::Object(change) = change else {
                            return false;
                        };
                        matches!(change.get("postWriteVerified"), Some(JsonValue::Bool(true)))
                    })
            })
        })
}

pub(crate) fn tool_batch_completed(
    exchanges: &[Result<ChatToolExchange, ipc_types::IpcError>],
) -> bool {
    exchanges.iter().all(|exchange| {
        exchange
            .as_ref()
            .is_ok_and(|exchange| matches!(exchange.result.outcome, ToolOutcome::Completed { .. }))
    })
}

/// A successful filename-discovery transport is not necessarily a resolved
/// discovery objective. Each requested selector must either return at least one
/// candidate or prove exhaustive coverage with no candidate. Incomplete empty
/// selectors remain refinable and must not advance plan progress or enter the
/// request-local reuse ledger as authoritative evidence.
pub(crate) fn find_paths_result_resolves_all_queries(
    call: &model_runtime::ParsedToolCall,
    exchange: &ChatToolExchange,
) -> bool {
    if call.name != "filesystem.find_paths" {
        return false;
    }
    let Some(requested) = call
        .arguments
        .get("queries")
        .and_then(serde_json::Value::as_array)
        .map(Vec::len)
    else {
        return false;
    };
    let ToolOutcome::Completed { content } = &exchange.result.outcome else {
        return false;
    };
    content.iter().any(|part| {
        let ToolResultPart::Json {
            value: JsonValue::Object(root),
        } = part
        else {
            return false;
        };
        if matches!(root.get("reused"), Some(JsonValue::Bool(true))) {
            return true;
        }
        let Some(JsonValue::Array(results)) = root.get("results") else {
            return false;
        };
        results.len() == requested && results.iter().all(|result| {
            let JsonValue::Object(fields) = result else {
                return false;
            };
            matches!(fields.get("matches"), Some(JsonValue::Array(matches)) if !matches.is_empty())
                || matches!(fields.get("complete"), Some(JsonValue::Bool(true)))
        })
    })
}

pub(crate) fn conversation_image_count(conversation: &Conversation) -> usize {
    conversation
        .messages
        .iter()
        .flat_map(|message| &message.parts)
        .map(|part| match part {
            MessagePart::Asset { .. } => 1,
            MessagePart::ToolResult { result } => match &result.outcome {
                ToolOutcome::Completed { content } => content
                    .iter()
                    .filter(|part| matches!(part, ToolResultPart::Asset { .. }))
                    .count(),
                ToolOutcome::Failed { .. } => 0,
            },
            _ => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use ipc_types::{IpcError, TaskActivityStatus, ToolCallId, ToolRequest, ToolResult};

    use super::*;

    #[test]
    fn directory_mutations_are_verified_and_partial_batches_are_detectable() {
        let exchange = directory_exchange(true);
        assert_eq!(applied_paths(&[Ok(exchange)]), ["outputs/monthly"]);
        assert!(applied_changes_verified(&[Ok(directory_exchange(true))]));
        assert!(!applied_changes_verified(&[Ok(directory_exchange(false))]));
        assert!(!tool_batch_completed(&[
            Ok(directory_exchange(true)),
            Err(IpcError::internal()),
        ]));
    }

    fn directory_exchange(verified: bool) -> ChatToolExchange {
        let call_id = ToolCallId("mkdir".to_owned());
        ChatToolExchange {
            request: ToolRequest {
                call_id: call_id.clone(),
                name: ipc_types::ToolName::FilesystemCreateDirectory,
                arguments: BTreeMap::from([(
                    "path".to_owned(),
                    JsonValue::String("outputs/monthly".to_owned()),
                )]),
            },
            result: ToolResult {
                call_id,
                outcome: ToolOutcome::Completed {
                    content: vec![ToolResultPart::Json {
                        value: JsonValue::Object(BTreeMap::from([(
                            "postCreateVerified".to_owned(),
                            JsonValue::Bool(verified),
                        )])),
                    }],
                },
            },
            activity: ipc_types::TaskActivity {
                id: "mkdir".to_owned(),
                kind: ipc_types::TaskActivityKind::FileWrite,
                title: "Created workspace directory".to_owned(),
                detail: None,
                status: TaskActivityStatus::Completed,
            },
            workspace_grant_id: None,
            undo_changes: Vec::new(),
            inspected_asset_id: None,
            computation_reuse_digest: None,
        }
    }
}
