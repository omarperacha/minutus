//! Paired non-executing results for bounded task calls awaiting an initial plan.

use super::{
    BatchEventSink, ChatToolExchange, Conversation, IpcError, IpcErrorCode, ParsedToolCall,
    RequestId, TaskActivityStatus, ToolOutcome, emit_finish, emit_request_start,
    prepare_invalid_tool_correction,
};

pub(crate) const PLAN_REQUIRED_OPERATION: &str = "orchestration.submit_plan";
pub(crate) const PLAN_PENDING_GUIDANCE: &str = "These task calls were deferred and were not executed. Submit orchestration.submit_plan alone in the next response. After acceptance, emit fresh task calls using the retained original inputs. Pending calls never execute automatically.";

pub(crate) fn plan_deferral_batch_is_bounded(calls: &[ParsedToolCall]) -> bool {
    !calls.is_empty()
        && calls.len() <= crate::app_state::MAX_CHAT_TOOL_CALLS
        && calls.iter().all(|call| {
            crate::chat_tools::tool_action_class(&call.name).is_some()
                && prepare_invalid_tool_correction(call).is_ok()
        })
        && calls
            .iter()
            .map(|call| &call.call_id)
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            == calls.len()
}

/// This boundary can only construct inert exchanges: it receives no effect owner.
pub(crate) fn deferred_plan_exchanges(
    calls: &[ParsedToolCall],
) -> Result<Vec<ChatToolExchange>, IpcError> {
    if !plan_deferral_batch_is_bounded(calls) {
        return Err(IpcError::invalid_request(
            "Unsafe or unknown pre-plan batch",
        ));
    }
    calls
        .iter()
        .map(|call| {
            let tool = prepare_invalid_tool_correction(call)?;
            Ok(ChatToolExchange {
                result: ipc_types::ToolResult {
                    call_id: tool.request.call_id.clone(),
                    outcome: ToolOutcome::Failed {
                        error: IpcError {
                            code: IpcErrorCode::InvalidRequest,
                            message: serde_json::json!({
                                "status": "deferred",
                                "reason": "plan_pending",
                                "requiredNextOperation": PLAN_REQUIRED_OPERATION,
                                "executed": false,
                                "guidance": PLAN_PENDING_GUIDANCE,
                            })
                            .to_string(),
                            retryable: true,
                        },
                    },
                },
                activity: ipc_types::TaskActivity {
                    title: "Deferred task call — not executed".to_owned(),
                    status: TaskActivityStatus::Partial,
                    detail: Some("Awaiting a separately submitted execution plan".to_owned()),
                    ..tool.activity
                },
                request: tool.request,
                workspace_grant_id: None,
                undo_changes: Vec::new(),
                inspected_asset_id: None,
                computation_reuse_digest: None,
            })
        })
        .collect()
}

pub(crate) fn emit_plan_deferrals(
    app: &tauri::AppHandle,
    request_id: &RequestId,
    sequence: &mut u32,
    conversation: &Conversation,
    exchanges: &[ChatToolExchange],
) {
    let mut events = BatchEventSink {
        app,
        request_id,
        sequence,
        conversation,
    };
    for exchange in exchanges {
        emit_request_start(&mut events, &exchange.request, &exchange.activity);
        emit_finish(&mut events, exchange);
    }
}

#[cfg(test)]
mod tests;
