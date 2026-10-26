//! Safe-boundary persistence for tool calls, results, and undo provenance.

use app_core::AppCore;
use ipc_types::{
    AssetId, Conversation, IpcError, Message, MessageId, MessagePart, MessageRole, RequestId,
    ToolName, ToolOutcome,
};

use crate::error_mapping::{app_core_error, current_timestamp};

use super::ChatToolExchange;

#[expect(
    clippy::too_many_arguments,
    reason = "one persistence transaction binds model output, executed exchanges, exact submitted assets and request-scoped diagnostics"
)]
pub(crate) fn persist_tool_exchanges(
    core: &AppCore,
    request_id: &RequestId,
    conversation: &mut Conversation,
    reasoning: String,
    content: String,
    exchanges: Vec<Result<ChatToolExchange, IpcError>>,
    submitted_assets: &[AssetId],
    binding_sha256: &str,
) -> Result<(), IpcError> {
    #[cfg(not(debug_assertions))]
    let _ = request_id;
    let exchanges = exchanges.into_iter().collect::<Result<Vec<_>, _>>()?;
    let inspected_asset_ids = exchanges
        .iter()
        .filter_map(|exchange| exchange.inspected_asset_id.clone())
        .collect::<Vec<_>>();
    let created_at = current_timestamp()?;
    let extraction = failed_delimited_table_repair_context(&exchanges).unwrap_or_else(|| {
        if reasoning.trim().is_empty() {
            content.clone()
        } else {
            reasoning.clone()
        }
    });
    let mut parts = Vec::new();
    if !reasoning.is_empty() {
        parts.push(MessagePart::Reasoning { text: reasoning });
    }
    if !content.is_empty() {
        parts.push(MessagePart::Text { text: content });
    }
    for exchange in &exchanges {
        parts.push(MessagePart::Activity {
            activity: exchange.activity.clone(),
        });
        parts.push(MessagePart::ToolCall {
            request: exchange.request.clone(),
        });
    }
    conversation.messages.push(Message {
        id: MessageId(uuid::Uuid::now_v7().to_string()),
        conversation_id: conversation.id.clone(),
        role: MessageRole::Assistant,
        parts,
        created_at: created_at.clone(),
    });
    for exchange in exchanges {
        let mut parts = vec![MessagePart::ToolResult {
            result: exchange.result,
        }];
        if !exchange.undo_changes.is_empty() {
            let grant_id = exchange.workspace_grant_id.ok_or_else(IpcError::internal)?;
            parts.push(MessagePart::FilesystemWriteRecord {
                write_call_id: exchange.request.call_id,
                grant_id,
                changes: exchange.undo_changes,
            });
        }
        conversation.messages.push(Message {
            id: MessageId(uuid::Uuid::now_v7().to_string()),
            conversation_id: conversation.id.clone(),
            role: MessageRole::Tool,
            parts,
            created_at: created_at.clone(),
        });
    }
    conversation.updated_at.clone_from(&created_at);
    if let Err(error) = core.save_conversation(conversation) {
        for asset_id in &inspected_asset_ids {
            let _ = core.discard_unreferenced_image(asset_id);
        }
        return Err(app_core_error(error));
    }
    let checkpoint = core.capture_rich_evidence_checkpoint(
        conversation,
        submitted_assets,
        binding_sha256,
        &extraction,
        &created_at,
    );
    #[cfg(debug_assertions)]
    core.record_development_tool_trace(&serde_json::json!({
        "event": "rich_evidence_checkpoint_candidate",
        "requestId": request_id.0,
        "conversationId": conversation.id.0,
        "status": match &checkpoint {
            Ok(true) => "committed",
            Ok(false) => "rejected_preserved_previous",
            Err(_) => "failed_preserved_previous",
        },
        "submittedAssetCount": submitted_assets.len(),
    }));
    #[cfg(not(debug_assertions))]
    let _ = checkpoint;
    Ok(())
}

fn failed_delimited_table_repair_context(exchanges: &[ChatToolExchange]) -> Option<String> {
    let exchange = exchanges.iter().find(|exchange| {
        exchange.request.name == ToolName::FilesystemWriteDelimitedTable
            && matches!(exchange.result.outcome, ToolOutcome::Failed { .. })
    })?;
    serde_json::to_string(&serde_json::json!({
        "purpose": "repair_the_failed_delimited_table_call_without_reloading_the_source",
        "authority": "untrusted_model_arguments_and_host_validation_result",
        "failedCall": {
            "operation": "filesystem.write_delimited_table",
            "arguments": &exchange.request.arguments,
        },
        "result": &exchange.result,
    }))
    .ok()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use ipc_types::{
        IpcErrorCode, JsonValue, TaskActivity, TaskActivityKind, TaskActivityStatus, ToolCallId,
        ToolRequest, ToolResult,
    };

    use super::*;

    #[test]
    fn failed_table_call_becomes_untrusted_structural_repair_context() -> Result<(), IpcError> {
        let call_id = ToolCallId("write-table".to_owned());
        let exchange = ChatToolExchange {
            request: ToolRequest {
                call_id: call_id.clone(),
                name: ToolName::FilesystemWriteDelimitedTable,
                arguments: BTreeMap::from([
                    (
                        "columns".to_owned(),
                        JsonValue::Array(vec![JsonValue::String("amount".to_owned())]),
                    ),
                    (
                        "rows".to_owned(),
                        JsonValue::Array(vec![JsonValue::Array(vec![JsonValue::Number(
                            392_807.0,
                        )])]),
                    ),
                ]),
            },
            result: ToolResult {
                call_id,
                outcome: ToolOutcome::Failed {
                    error: IpcError {
                        code: IpcErrorCode::InvalidRequest,
                        message: "missing path".to_owned(),
                        retryable: true,
                    },
                },
            },
            activity: TaskActivity {
                id: "write-table".to_owned(),
                kind: TaskActivityKind::FileWrite,
                title: "Write table".to_owned(),
                detail: None,
                status: TaskActivityStatus::Failed,
            },
            workspace_grant_id: None,
            undo_changes: Vec::new(),
            inspected_asset_id: None,
            computation_reuse_digest: None,
        };
        let context =
            failed_delimited_table_repair_context(&[exchange]).ok_or_else(IpcError::internal)?;
        assert!(context.contains("untrusted_model_arguments"));
        assert!(context.contains("392807"));
        assert!(context.contains("missing path"));
        Ok(())
    }
}
