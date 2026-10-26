use super::{
    BatchEventSink, ChatToolExchange, Conversation, IpcError, IpcErrorCode, ParsedToolCall,
    PreparedToolCorrection, RequestId, TaskActivityStatus, ToolOutcome, chat_tool_contract,
    emit_correction_start, emit_finish, inactive_tool_call_is_safely_bounded,
    prepare_inactive_tool_correction, prepare_invalid_tool_correction,
    prepare_unknown_tool_correction, unknown_tool_call_is_safely_bounded,
};

pub(crate) fn reject_inactive_tool_batch(
    app: &tauri::AppHandle,
    request_id: &RequestId,
    sequence: &mut u32,
    conversation: &Conversation,
    calls: &[ParsedToolCall],
    available_tools: &[String],
) -> Vec<Result<ChatToolExchange, IpcError>> {
    let prepared = calls
        .iter()
        .map(|call| prepare_inactive_tool_correction(call, available_tools))
        .collect::<Vec<_>>();
    if prepared.iter().any(Result::is_err) {
        return prepared
            .into_iter()
            .map(|item| item.and_then(|_| Err(IpcError::internal())))
            .collect();
    }
    let mut events = BatchEventSink {
        app,
        request_id,
        sequence,
        conversation,
    };
    prepared
        .into_iter()
        .filter_map(Result::ok)
        .map(|tool| {
            emit_correction_start(&mut events, &tool);
            let exchange = tool_availability_correction_exchange(&tool, available_tools);
            emit_finish(&mut events, &exchange);
            Ok(exchange)
        })
        .collect()
}

pub(crate) fn reject_final_response_tool_batch(
    app: &tauri::AppHandle,
    request_id: &RequestId,
    sequence: &mut u32,
    conversation: &Conversation,
    calls: &[ParsedToolCall],
) -> Vec<Result<ChatToolExchange, IpcError>> {
    let prepared = calls
        .iter()
        .map(|call| prepare_inactive_tool_correction(call, &[]))
        .collect::<Vec<_>>();
    if prepared.iter().any(Result::is_err) {
        return prepared
            .into_iter()
            .map(|item| item.and_then(|_| Err(IpcError::internal())))
            .collect();
    }
    let mut events = BatchEventSink {
        app,
        request_id,
        sequence,
        conversation,
    };
    prepared
        .into_iter()
        .filter_map(Result::ok)
        .map(|tool| {
            emit_correction_start(&mut events, &tool);
            let exchange = final_response_correction_exchange(&tool);
            emit_finish(&mut events, &exchange);
            Ok(exchange)
        })
        .collect()
}

pub(crate) fn invalid_tool_batch_repairable(calls: &[ParsedToolCall]) -> bool {
    matches!(calls, [call] if prepare_invalid_tool_correction(call).is_ok())
}

pub(crate) fn mixed_read_only_invalid_batch_repairable(
    calls: &[ParsedToolCall],
    invalid_index: usize,
) -> bool {
    calls.len() > 1
        && invalid_index < calls.len()
        && calls.iter().all(|call| {
            is_read_only_tool(&call.name) && prepare_invalid_tool_correction(call).is_ok()
        })
}

pub(crate) fn plan_prefixed_invalid_operation_batch_repairable(
    calls: &[ParsedToolCall],
    invalid_index: usize,
) -> bool {
    matches!(calls, [plan, operation]
        if invalid_index == 1
            && plan.name == "orchestration.submit_plan"
            && prepare_invalid_tool_correction(operation).is_ok())
}

fn is_read_only_tool(name: &str) -> bool {
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
            | "computation.run"
            | "computation.run_project"
            | "web.retrieve"
            | "web.search"
    )
}

pub(crate) fn unknown_tool_batch_repairable(calls: &[ParsedToolCall]) -> bool {
    !calls.is_empty() && calls.iter().all(unknown_tool_call_is_safely_bounded)
}

pub(crate) fn inactive_tool_batch_repairable(calls: &[ParsedToolCall]) -> bool {
    !calls.is_empty() && calls.iter().all(inactive_tool_call_is_safely_bounded)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn reject_unknown_tool_batch(
    app: &tauri::AppHandle,
    request_id: &RequestId,
    sequence: &mut u32,
    conversation: &Conversation,
    calls: &[ParsedToolCall],
    available_tools: &[String],
) -> Vec<Result<ChatToolExchange, IpcError>> {
    let prepared = calls
        .iter()
        .map(|call| prepare_unknown_tool_correction(call, available_tools))
        .collect::<Vec<_>>();
    if prepared.iter().any(Result::is_err) {
        return prepared
            .into_iter()
            .map(|item| item.and_then(|_| Err(IpcError::internal())))
            .collect();
    }
    let mut events = BatchEventSink {
        app,
        request_id,
        sequence,
        conversation,
    };
    prepared
        .into_iter()
        .filter_map(Result::ok)
        .map(|tool| {
            emit_correction_start(&mut events, &tool);
            let exchange = tool_availability_correction_exchange(&tool, available_tools);
            emit_finish(&mut events, &exchange);
            Ok(exchange)
        })
        .collect()
}

pub(crate) fn complete_control_call(
    app: &tauri::AppHandle,
    request_id: &RequestId,
    sequence: &mut u32,
    conversation: &Conversation,
    call: &ParsedToolCall,
    value: &serde_json::Value,
) -> Result<ChatToolExchange, IpcError> {
    let tool = prepare_invalid_tool_correction(call)?;
    let mut events = BatchEventSink {
        app,
        request_id,
        sequence,
        conversation,
    };
    emit_correction_start(&mut events, &tool);
    let exchange = ChatToolExchange {
        request: tool.request.clone(),
        result: ipc_types::ToolResult {
            call_id: tool.request.call_id.clone(),
            outcome: ToolOutcome::Completed {
                content: vec![ipc_types::ToolResultPart::Json {
                    value: crate::chat_tools::filesystem::ipc_json_value(value)?,
                }],
            },
        },
        activity: ipc_types::TaskActivity {
            status: TaskActivityStatus::Completed,
            detail: Some(
                match call.name.as_str() {
                    "orchestration.complete_stage" => "Semantic plan stage completed",
                    "orchestration.search_tools" => "Task-tool schemas loaded",
                    "orchestration.recall" => "Historical context recalled",
                    "orchestration.submit_plan" if value["planUnchanged"] == true => {
                        "Execution plan already accepted"
                    }
                    "orchestration.submit_plan" if value["mode"] == "revise" => {
                        "Execution plan revision accepted"
                    }
                    "orchestration.submit_plan" => "Execution plan accepted",
                    _ => "Orchestration update accepted",
                }
                .to_owned(),
            ),
            ..tool.activity
        },
        workspace_grant_id: None,
        undo_changes: Vec::new(),
        inspected_asset_id: None,
        computation_reuse_digest: None,
    };
    emit_finish(&mut events, &exchange);
    Ok(exchange)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn reject_invalid_tool_batch(
    app: &tauri::AppHandle,
    request_id: &RequestId,
    sequence: &mut u32,
    conversation: &Conversation,
    calls: &[ParsedToolCall],
    validation_error: &IpcError,
) -> Vec<Result<ChatToolExchange, IpcError>> {
    let prepared = calls
        .iter()
        .map(prepare_invalid_tool_correction)
        .collect::<Vec<_>>();
    if prepared.iter().any(Result::is_err) {
        return prepared
            .into_iter()
            .map(|item| item.and_then(|_| Err(IpcError::internal())))
            .collect();
    }
    let mut events = BatchEventSink {
        app,
        request_id,
        sequence,
        conversation,
    };
    prepared
        .into_iter()
        .filter_map(Result::ok)
        .map(|tool| {
            emit_correction_start(&mut events, &tool);
            let exchange = tool_argument_correction_exchange(&tool, validation_error);
            emit_finish(&mut events, &exchange);
            Ok(exchange)
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn reject_plan_deviation_batch(
    app: &tauri::AppHandle,
    request_id: &RequestId,
    sequence: &mut u32,
    conversation: &Conversation,
    calls: &[ParsedToolCall],
    validation_error: &IpcError,
) -> Vec<Result<ChatToolExchange, IpcError>> {
    let prepared = calls
        .iter()
        .map(prepare_invalid_tool_correction)
        .collect::<Vec<_>>();
    if prepared.iter().any(Result::is_err) {
        return prepared
            .into_iter()
            .map(|item| item.and_then(|_| Err(IpcError::internal())))
            .collect();
    }
    let mut events = BatchEventSink {
        app,
        request_id,
        sequence,
        conversation,
    };
    prepared
        .into_iter()
        .filter_map(Result::ok)
        .map(|tool| {
            emit_correction_start(&mut events, &tool);
            let exchange = plan_deviation_correction_exchange(&tool, validation_error);
            emit_finish(&mut events, &exchange);
            Ok(exchange)
        })
        .collect()
}

pub(super) fn plan_deviation_correction_exchange(
    tool: &PreparedToolCorrection,
    validation_error: &IpcError,
) -> ChatToolExchange {
    ChatToolExchange {
        request: tool.request.clone(),
        result: ipc_types::ToolResult {
            call_id: tool.request.call_id.clone(),
            outcome: ToolOutcome::Failed {
                error: validation_error.clone(),
            },
        },
        activity: ipc_types::TaskActivity {
            status: TaskActivityStatus::Partial,
            detail: Some("Minutus is aligning this operation with its accepted plan".to_owned()),
            ..tool.activity.clone()
        },
        workspace_grant_id: None,
        undo_changes: Vec::new(),
        inspected_asset_id: None,
        computation_reuse_digest: None,
    }
}

pub(super) fn tool_argument_correction_exchange(
    tool: &PreparedToolCorrection,
    validation_error: &IpcError,
) -> ChatToolExchange {
    let expected = chat_tool_contract(&tool.call.name)
        .map(|(_, arguments)| arguments.join(", "))
        .unwrap_or_default();
    let message = match validation_error.code {
        IpcErrorCode::PermissionDenied => format!(
            "The tool was not executed because the exact operation was not authorized: {}. Do not retry unchanged; use an authorized scope or ask the user for the required permission.",
            validation_error.message
        ),
        IpcErrorCode::ScopeCorrection => format!(
            "The tool was not executed because its resource is outside the active user-authorized scope: {}. Select an advertised workspace/resource within scope, or ask the user before expanding scope.",
            validation_error.message
        ),
        IpcErrorCode::PlanMismatch => format!(
            "The tool was not executed because it does not advance the current semantic plan step: {}. Return to the current step or submit the smallest valid revision.",
            validation_error.message
        ),
        _ => format!(
            "The tool was not executed: {}. Retry once with corrected arguments. Tool: {}. Expected top-level arguments: {}.",
            validation_error.message, tool.call.name, expected
        ),
    };
    ChatToolExchange {
        request: tool.request.clone(),
        result: ipc_types::ToolResult {
            call_id: tool.request.call_id.clone(),
            outcome: ToolOutcome::Failed {
                error: IpcError {
                    code: validation_error.code.clone(),
                    message,
                    retryable: validation_error.code != IpcErrorCode::PermissionDenied,
                },
            },
        },
        activity: ipc_types::TaskActivity {
            status: TaskActivityStatus::Partial,
            detail: Some(
                if tool.call.name == "orchestration.submit_plan" {
                    "Plan submission was rejected"
                } else {
                    "Operation was not executed"
                }
                .to_owned(),
            ),
            ..tool.activity.clone()
        },
        workspace_grant_id: None,
        undo_changes: Vec::new(),
        inspected_asset_id: None,
        computation_reuse_digest: None,
    }
}

pub(super) fn tool_availability_correction_exchange(
    tool: &PreparedToolCorrection,
    available_tools: &[String],
) -> ChatToolExchange {
    let rejection = if available_tools.contains(&tool.call.name) {
        format!(
            "The requested tool '{}' is available, but this batch was not executed because another requested tool is unavailable. Its arguments have not been validated.",
            tool.call.name
        )
    } else {
        format!(
            "The requested tool '{}' is unavailable in this projection and was not executed.",
            tool.call.name
        )
    };
    ChatToolExchange {
        request: tool.request.clone(),
        result: ipc_types::ToolResult {
            call_id: tool.request.call_id.clone(),
            outcome: ToolOutcome::Failed {
                error: IpcError {
                    code: IpcErrorCode::InvalidRequest,
                    message: format!(
                        "{} Continue the same step with one of the currently advertised tools: {}. If orchestration.search_tools is advertised, use it to load an omitted task schema. Submit an initial plan when planning is required; revise an existing semantic plan only for a material contract change. A legacy forecast may be revised only when its control is advertised. Ask the user only when a required value or authority cannot be recovered safely.",
                        rejection,
                        available_tools.join(", ")
                    ),
                    retryable: true,
                },
            },
        },
        activity: ipc_types::TaskActivity {
            status: TaskActivityStatus::Partial,
            detail: Some(
                if tool.call.name == "orchestration.submit_plan" {
                    "Plan submission was rejected"
                } else {
                    "Operation was not executed"
                }
                .to_owned(),
            ),
            ..tool.activity.clone()
        },
        workspace_grant_id: None,
        undo_changes: Vec::new(),
        inspected_asset_id: None,
        computation_reuse_digest: None,
    }
}

pub(super) fn final_response_correction_exchange(
    tool: &PreparedToolCorrection,
) -> ChatToolExchange {
    ChatToolExchange {
        request: tool.request.clone(),
        result: ipc_types::ToolResult {
            call_id: tool.request.call_id.clone(),
            outcome: ToolOutcome::Failed {
                error: IpcError {
                    code: IpcErrorCode::InvalidRequest,
                    message: "The requested operation was not executed because the verified outcome is complete and the tool surface is intentionally closed. Do not call any task or orchestration tool, revise the plan, or search for tools. Respond to the user now in plain text using the established result retained in context."
                        .to_owned(),
                    retryable: true,
                },
            },
        },
        activity: ipc_types::TaskActivity {
            status: TaskActivityStatus::Partial,
            detail: Some("Minutus is preparing the verified final response".to_owned()),
            ..tool.activity.clone()
        },
        workspace_grant_id: None,
        undo_changes: Vec::new(),
        inspected_asset_id: None,
        computation_reuse_digest: None,
    }
}
