//! Validated tool dispatch and trusted activity reporting.

use std::sync::atomic::AtomicBool;

use app_core::AppCore;
use ipc_types::{
    Conversation, IpcError, TaskActivityStatus, ToolOutcome, ToolRequest, ToolResult,
    ToolResultPart, ValidateIpc,
};
use model_runtime::ParsedToolCall;

use crate::chat_runtime::tool_activity;

use super::{
    ChatToolExchange, ChatToolWorkspace, artifact_transform, collect_evidence, computation,
    filesystem::{ipc_json_value, run_chat_filesystem_tool},
    validation::{chat_tool_contract, required_string_argument, validated_chat_tool_request},
};

const MAX_UNKNOWN_TOOL_NAME_BYTES: usize = 128;

#[derive(Clone)]
pub(super) struct PreparedChatTool {
    pub(super) call: ParsedToolCall,
    pub(super) request: ipc_types::ToolRequest,
    pub(super) workspace: Option<ChatToolWorkspace>,
    pub(super) activity: ipc_types::TaskActivity,
}

pub(super) struct PreparedToolCorrection {
    pub(super) call: ParsedToolCall,
    pub(super) request: ToolRequest,
    pub(super) activity: ipc_types::TaskActivity,
}

pub(super) fn prepare_invalid_tool_correction(
    call: &ParsedToolCall,
) -> Result<PreparedToolCorrection, IpcError> {
    if call.recipient != call.name {
        return Err(IpcError::invalid_request(
            "The model returned an invalid tool recipient",
        ));
    }
    let (name, _) = chat_tool_contract(&call.name)
        .ok_or_else(|| IpcError::invalid_request("The model requested an unavailable tool"))?;
    let request = ToolRequest {
        call_id: ipc_types::ToolCallId(call.call_id.clone()),
        name,
        arguments: call
            .arguments
            .iter()
            .map(|(name, value)| Ok((name.clone(), ipc_json_value(value)?)))
            .collect::<Result<_, IpcError>>()?,
    };
    request.validate()?;
    Ok(PreparedToolCorrection {
        activity: tool_activity(&request.call_id.0, &call.name),
        call: call.clone(),
        request,
    })
}

pub(super) fn unknown_tool_call_is_safely_bounded(call: &ParsedToolCall) -> bool {
    chat_tool_contract(&call.name).is_none() && tool_call_envelope_is_safely_bounded(call)
}

pub(super) fn inactive_tool_call_is_safely_bounded(call: &ParsedToolCall) -> bool {
    chat_tool_contract(&call.name).is_some() && tool_call_envelope_is_safely_bounded(call)
}

fn tool_call_envelope_is_safely_bounded(call: &ParsedToolCall) -> bool {
    if call.recipient != call.name || !safe_tool_name(&call.name) {
        return false;
    }
    let arguments = call
        .arguments
        .iter()
        .map(|(name, value)| Ok((name.clone(), ipc_json_value(value)?)))
        .collect::<Result<_, IpcError>>();
    arguments.is_ok_and(|arguments| {
        ToolRequest {
            call_id: ipc_types::ToolCallId(call.call_id.clone()),
            name: ipc_types::ToolName::OrchestrationToolCorrection,
            arguments,
        }
        .validate()
        .is_ok()
    })
}

pub(super) fn prepare_unknown_tool_correction(
    call: &ParsedToolCall,
    available_tools: &[String],
) -> Result<PreparedToolCorrection, IpcError> {
    if !unknown_tool_call_is_safely_bounded(call) {
        return Err(IpcError::invalid_request(
            "The model returned an unsafe tool call",
        ));
    }
    prepare_tool_availability_correction(call, available_tools)
}

pub(super) fn prepare_inactive_tool_correction(
    call: &ParsedToolCall,
    available_tools: &[String],
) -> Result<PreparedToolCorrection, IpcError> {
    if !inactive_tool_call_is_safely_bounded(call) {
        return Err(IpcError::invalid_request(
            "The model returned an unsafe inactive tool call",
        ));
    }
    prepare_tool_availability_correction(call, available_tools)
}

fn prepare_tool_availability_correction(
    call: &ParsedToolCall,
    available_tools: &[String],
) -> Result<PreparedToolCorrection, IpcError> {
    let request = ToolRequest {
        call_id: ipc_types::ToolCallId(call.call_id.clone()),
        name: ipc_types::ToolName::OrchestrationToolCorrection,
        arguments: std::collections::BTreeMap::from([
            (
                "availableTools".to_owned(),
                ipc_types::JsonValue::Array(
                    available_tools
                        .iter()
                        .cloned()
                        .map(ipc_types::JsonValue::String)
                        .collect(),
                ),
            ),
            (
                "requestedTool".to_owned(),
                ipc_types::JsonValue::String(call.name.clone()),
            ),
        ]),
    };
    request.validate()?;
    Ok(PreparedToolCorrection {
        activity: tool_activity(&request.call_id.0, "orchestration.tool_correction"),
        call: call.clone(),
        request,
    })
}

fn safe_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_UNKNOWN_TOOL_NAME_BYTES
        && name.split('.').all(|part| {
            let mut bytes = part.bytes();
            bytes
                .next()
                .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
                && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
}

pub(super) fn prepare_chat_tool(
    workspaces: &[ChatToolWorkspace],
    call: &ParsedToolCall,
) -> Result<PreparedChatTool, IpcError> {
    let request = validated_chat_tool_request(call, workspaces)?;
    let workspace = if matches!(
        call.name.as_str(),
        "computation.run"
            | "web.retrieve"
            | "web.search"
            | "x402.request"
            | "wallet.request_transfer"
            | "contract.inspect"
            | "contract.read"
            | "contract.request_workflow"
    ) {
        None
    } else {
        let workspace_alias = required_string_argument(&call.arguments, "workspace")?;
        workspaces
            .iter()
            .find(|workspace| workspace.alias == workspace_alias)
            .ok_or_else(|| {
                IpcError::invalid_request("The requested project workspace is unavailable")
            })?
            .clone()
            .into()
    };
    Ok(PreparedChatTool {
        activity: tool_activity(&request.call_id.0, &call.name),
        call: call.clone(),
        request,
        workspace,
    })
}

#[expect(
    clippy::too_many_lines,
    reason = "dispatch keeps result and activity construction exhaustive across the closed tool set"
)]
pub(super) fn execute_prepared_chat_tool(
    app: &tauri::AppHandle,
    core: &AppCore,
    conversation: &Conversation,
    prepared: PreparedChatTool,
    image_slot_available: bool,
    cancellation_requested: &std::sync::Arc<AtomicBool>,
) -> ChatToolExchange {
    let PreparedChatTool {
        call,
        request,
        workspace,
        mut activity,
    } = prepared;
    #[cfg(debug_assertions)]
    if let Some(error) = crate::chat_runtime::evaluation_tool_denial(app, &call.name) {
        activity.status = TaskActivityStatus::Failed;
        return ChatToolExchange {
            request: request.clone(),
            result: ToolResult {
                call_id: request.call_id,
                outcome: ToolOutcome::Failed { error },
            },
            activity,
            workspace_grant_id: None,
            undo_changes: Vec::new(),
            inspected_asset_id: None,
            computation_reuse_digest: None,
        };
    }
    if matches!(
        call.name.as_str(),
        "contract.inspect" | "contract.read" | "contract.request_workflow"
    ) {
        let outcome = if call.name == "contract.request_workflow" {
            super::contracts::execute_write(app, conversation, &call, cancellation_requested)
        } else {
            super::contracts::execute(app, &call, cancellation_requested)
        }
        .unwrap_or_else(|mut error| {
            error.retryable = false;
            ToolOutcome::Failed { error }
        });
        activity.status = match &outcome {
            ToolOutcome::Completed { .. } => TaskActivityStatus::Completed,
            ToolOutcome::Failed { .. } => TaskActivityStatus::Failed,
        };
        activity.detail = Some(
            if call.name == "contract.request_workflow" {
                "Qualified fixed workflow · independent native approval per write"
            } else {
                "Unsigned RPC · separate native outbound review · unverified ABI"
            }
            .into(),
        );
        return ChatToolExchange {
            request: request.clone(),
            result: ToolResult {
                call_id: request.call_id,
                outcome,
            },
            activity,
            workspace_grant_id: None,
            undo_changes: Vec::new(),
            inspected_asset_id: None,
            computation_reuse_digest: None,
        };
    }
    if call.name == "wallet.request_transfer" {
        let outcome =
            super::wallet_transfer::execute(app, core, conversation, &call, cancellation_requested)
                .unwrap_or_else(|mut error| {
                    error.retryable = false;
                    ToolOutcome::Failed { error }
                });
        activity.status = match &outcome {
            ToolOutcome::Completed { .. } => TaskActivityStatus::Completed,
            ToolOutcome::Failed { .. } => TaskActivityStatus::Failed,
        };
        activity.detail =
            Some("Transfer request · separate native approval and fresh password".into());
        return ChatToolExchange {
            request: request.clone(),
            result: ToolResult {
                call_id: request.call_id,
                outcome,
            },
            activity,
            workspace_grant_id: None,
            undo_changes: Vec::new(),
            inspected_asset_id: None,
            computation_reuse_digest: None,
        };
    }
    if call.name == "x402.request" {
        let outcome = super::x402::execute(app, core, conversation, &call, cancellation_requested)
            .unwrap_or_else(|error| ToolOutcome::Failed { error });
        let result = ToolResult {
            call_id: request.call_id.clone(),
            outcome,
        };
        activity.status = match result.outcome {
            ToolOutcome::Completed { .. } => TaskActivityStatus::Completed,
            ToolOutcome::Failed { .. } => TaskActivityStatus::Failed,
        };
        activity.detail = Some(
            if call
                .arguments
                .get("operation")
                .and_then(serde_json::Value::as_str)
                == Some("discover")
            {
                "x402 Bazaar discovery · no payment"
            } else {
                "x402 resource · separate native payment approval"
            }
            .to_owned(),
        );
        return ChatToolExchange {
            request,
            result,
            activity,
            workspace_grant_id: None,
            undo_changes: Vec::new(),
            inspected_asset_id: None,
            computation_reuse_digest: None,
        };
    }
    if call.name == "web.retrieve" {
        let execution = super::web::execute(
            app,
            core,
            conversation,
            &call,
            image_slot_available,
            cancellation_requested,
        );
        let (outcome, asset_id, injection_signal_count) = match execution {
            Ok(execution) => (
                execution.outcome,
                execution.asset_id,
                execution.injection_signal_count,
            ),
            Err(error) => (ToolOutcome::Failed { error }, None, 0),
        };
        let result = ToolResult {
            call_id: request.call_id.clone(),
            outcome,
        };
        activity.status = match &result.outcome {
            ToolOutcome::Completed { .. } => TaskActivityStatus::Compacted,
            ToolOutcome::Failed { .. } => TaskActivityStatus::Failed,
        };
        activity.detail = Some(if injection_signal_count == 0 {
            "External network · fresh one-time approval".to_owned()
        } else {
            "External network · remote prompt-injection signals detected".to_owned()
        });
        return ChatToolExchange {
            request,
            result,
            activity,
            workspace_grant_id: None,
            undo_changes: Vec::new(),
            inspected_asset_id: asset_id,
            computation_reuse_digest: None,
        };
    }
    if call.name == "web.search" {
        let execution =
            super::web_search::execute(app, core, conversation, &call, cancellation_requested);
        let (outcome, injection_signal_count) = match execution {
            Ok(execution) => (execution.outcome, execution.injection_signal_count),
            Err(error) => (ToolOutcome::Failed { error }, 0),
        };
        let result = ToolResult {
            call_id: request.call_id.clone(),
            outcome,
        };
        activity.status = match &result.outcome {
            ToolOutcome::Completed { .. } => TaskActivityStatus::Compacted,
            ToolOutcome::Failed { .. } => TaskActivityStatus::Failed,
        };
        activity.detail = Some(if injection_signal_count == 0 {
            "Brave Search · fresh one-time approval".to_owned()
        } else {
            "Brave Search · prompt-injection signals detected".to_owned()
        });
        return ChatToolExchange {
            request,
            result,
            activity,
            workspace_grant_id: None,
            undo_changes: Vec::new(),
            inspected_asset_id: None,
            computation_reuse_digest: None,
        };
    }
    if matches!(
        call.name.as_str(),
        "computation.run" | "computation.run_project"
    ) {
        let workspace_ref = workspace.as_ref();
        let execution = computation::execute(
            core,
            conversation,
            workspace_ref,
            &call,
            cancellation_requested,
        );
        let (outcome, reuse_digest) = match execution {
            Ok(execution) => (execution.outcome, Some(execution.reuse_digest)),
            Err(error) => (ToolOutcome::Failed { error }, None),
        };
        let result = ToolResult {
            call_id: request.call_id.clone(),
            outcome,
        };
        activity.status = match &result.outcome {
            ToolOutcome::Completed { content } if content.iter().any(result_part_is_compacted) => {
                TaskActivityStatus::Compacted
            }
            ToolOutcome::Completed { .. } => TaskActivityStatus::Completed,
            ToolOutcome::Failed { .. } => TaskActivityStatus::Failed,
        };
        let display_name = workspace
            .as_ref()
            .map_or("Local sandbox (no project access)", |workspace| {
                workspace.display_name.as_str()
            });
        activity.detail = Some(format!("{display_name} · {}", call.name));
        return ChatToolExchange {
            request,
            result,
            activity,
            workspace_grant_id: workspace
                .as_ref()
                .map(|workspace| workspace.grant_id.clone()),
            undo_changes: Vec::new(),
            inspected_asset_id: None,
            computation_reuse_digest: reuse_digest,
        };
    }
    let requests_image = call.name == "filesystem.inspect_image"
        || (call.name == "filesystem.collect_evidence"
            && collect_evidence::requests_image(&call.arguments).unwrap_or(false));
    let execution = if requests_image && !image_slot_available {
        Err(IpcError::payload_too_large(
            "The conversation already contains the maximum number of images",
        ))
    } else if let Some(workspace) = workspace.as_ref() {
        if call.name == "filesystem.write_delimited_table" {
            artifact_transform::materialize_delimited_table(conversation, &call).and_then(
                |write_call| {
                    run_chat_filesystem_tool(core, &conversation.project_id, workspace, &write_call)
                },
            )
        } else if call.name == "orchestration.transform_artifact" {
            artifact_transform::materialize(conversation, &call).and_then(|write_call| {
                run_chat_filesystem_tool(core, &conversation.project_id, workspace, &write_call)
            })
        } else {
            run_chat_filesystem_tool(core, &conversation.project_id, workspace, &call)
        }
    } else {
        Err(IpcError::internal())
    };
    let (result, undo_changes, inspected_asset_id) = match execution {
        Ok(execution) => {
            let mut content = vec![ToolResultPart::Json {
                value: execution.value,
            }];
            if let Some(asset_id) = &execution.inspected_asset_id {
                content.push(ToolResultPart::Asset {
                    asset_id: asset_id.clone(),
                });
            }
            (
                ToolResult {
                    call_id: request.call_id.clone(),
                    outcome: ToolOutcome::Completed { content },
                },
                execution.undo_changes,
                execution.inspected_asset_id,
            )
        }
        Err(error) => (
            ToolResult {
                call_id: request.call_id.clone(),
                outcome: ToolOutcome::Failed { error },
            },
            Vec::new(),
            None,
        ),
    };
    activity.status = match &result.outcome {
        ToolOutcome::Completed { content } if content.iter().any(result_part_is_compacted) => {
            TaskActivityStatus::Compacted
        }
        ToolOutcome::Completed { .. } => TaskActivityStatus::Completed,
        ToolOutcome::Failed { .. } => TaskActivityStatus::Failed,
    };
    activity.detail = Some(format!(
        "{} · {}",
        workspace
            .as_ref()
            .map_or("Unavailable workspace", |workspace| workspace
                .display_name
                .as_str()),
        call.name
    ));
    ChatToolExchange {
        request,
        result,
        activity,
        workspace_grant_id: workspace.map(|workspace| workspace.grant_id),
        undo_changes,
        inspected_asset_id,
        computation_reuse_digest: None,
    }
}

fn result_part_is_compacted(part: &ToolResultPart) -> bool {
    let ToolResultPart::Json { value } = part else {
        return false;
    };
    json_reports_truncation(value) || json_is_compact_reference(value)
}

fn json_is_compact_reference(value: &ipc_types::JsonValue) -> bool {
    let ipc_types::JsonValue::Object(fields) = value else {
        return false;
    };
    let Some(ipc_types::JsonValue::Object(reference)) = fields.get("reference") else {
        return false;
    };
    matches!(reference.get("kind"), Some(ipc_types::JsonValue::String(kind)) if kind == "compactToolResult")
}

fn json_reports_truncation(value: &ipc_types::JsonValue) -> bool {
    match value {
        ipc_types::JsonValue::Object(fields) => {
            matches!(
                fields.get("truncated"),
                Some(ipc_types::JsonValue::Bool(true))
            ) || fields.values().any(json_reports_truncation)
        }
        ipc_types::JsonValue::Array(values) => values.iter().any(json_reports_truncation),
        ipc_types::JsonValue::Null
        | ipc_types::JsonValue::Bool(_)
        | ipc_types::JsonValue::Number(_)
        | ipc_types::JsonValue::String(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn bounded_truncated_results_are_labeled_compacted() {
        let compacted = ToolResultPart::Json {
            value: ipc_types::JsonValue::Object(BTreeMap::from([(
                "truncated".to_owned(),
                ipc_types::JsonValue::Bool(true),
            )])),
        };
        let complete = ToolResultPart::Json {
            value: ipc_types::JsonValue::Object(BTreeMap::from([(
                "truncated".to_owned(),
                ipc_types::JsonValue::Bool(false),
            )])),
        };
        assert!(result_part_is_compacted(&compacted));
        assert!(!result_part_is_compacted(&complete));

        let nested = ToolResultPart::Json {
            value: ipc_types::JsonValue::Object(BTreeMap::from([(
                "results".to_owned(),
                ipc_types::JsonValue::Array(vec![ipc_types::JsonValue::Object(BTreeMap::from([
                    ("truncated".to_owned(), ipc_types::JsonValue::Bool(true)),
                ]))]),
            )])),
        };
        assert!(result_part_is_compacted(&nested));
    }
}
