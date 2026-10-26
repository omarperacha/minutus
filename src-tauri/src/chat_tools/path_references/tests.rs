use std::collections::BTreeMap;

use ipc_types::{
    TaskActivity, TaskActivityStatus, ToolCallId, ToolRequest, ToolResult, ToolResultPart,
};

use super::*;

fn discovery(path: &str, workspace: &str) -> ChatToolExchange {
    ChatToolExchange {
        request: ToolRequest {
            call_id: ToolCallId("discover".to_owned()),
            name: ipc_types::ToolName::FilesystemFindPaths,
            arguments: BTreeMap::from([(
                "workspace".to_owned(),
                JsonValue::String(workspace.to_owned()),
            )]),
        },
        result: ToolResult {
            call_id: ToolCallId("discover".to_owned()),
            outcome: ToolOutcome::Completed {
                content: vec![ToolResultPart::Json {
                    value: JsonValue::Object(BTreeMap::from([(
                        "results".to_owned(),
                        JsonValue::Array(vec![JsonValue::Object(BTreeMap::from([(
                            "matches".to_owned(),
                            JsonValue::Array(vec![JsonValue::Object(BTreeMap::from([(
                                "relativePath".to_owned(),
                                JsonValue::String(path.to_owned()),
                            )]))]),
                        )]))]),
                    )])),
                }],
            },
        },
        activity: TaskActivity {
            id: "discover".to_owned(),
            kind: ipc_types::TaskActivityKind::FileRead,
            title: "Discover".to_owned(),
            status: TaskActivityStatus::Completed,
            detail: None,
        },
        workspace_grant_id: None,
        undo_changes: Vec::new(),
        inspected_asset_id: None,
        computation_reuse_digest: None,
    }
}

fn call(path: &str, workspace: &str) -> ParsedToolCall {
    ParsedToolCall {
        call_id: "read".to_owned(),
        recipient: "filesystem.inspect_image".to_owned(),
        name: "filesystem.inspect_image".to_owned(),
        arguments: BTreeMap::from([
            (
                "path".to_owned(),
                serde_json::Value::String(path.to_owned()),
            ),
            (
                "workspace".to_owned(),
                serde_json::Value::String(workspace.to_owned()),
            ),
        ]),
    }
}

#[test]
fn discovery_reference_resolves_to_the_exact_bound_path() -> Result<(), Box<dyn std::error::Error>>
{
    let mut ledger = PathReferenceLedger::default();
    let mut exchange = discovery("monthly cashflow/source 1.png", "primary");
    ledger.annotate_exchange(&mut exchange);

    let ToolOutcome::Completed { content } = &exchange.result.outcome else {
        return Err("discovery did not complete".into());
    };
    let ToolResultPart::Json {
        value: JsonValue::Object(root),
    } = &content[0]
    else {
        return Err("discovery did not return JSON".into());
    };
    let JsonValue::Array(results) = &root["results"] else {
        return Err("discovery results are absent".into());
    };
    let JsonValue::Object(result) = &results[0] else {
        return Err("discovery result is not an object".into());
    };
    let JsonValue::Array(matches) = &result["matches"] else {
        return Err("discovery matches are absent".into());
    };
    let JsonValue::Object(entry) = &matches[0] else {
        return Err("discovery match is not an object".into());
    };
    assert_eq!(
        entry.get("pathRef"),
        Some(&JsonValue::String(ledger.identity_for(&ReferencedPath {
            workspace: "primary".to_owned(),
            relative_path: "monthly cashflow/source 1.png".to_owned()
        })))
    );

    let reference = ledger
        .by_id
        .keys()
        .next()
        .ok_or("missing reference")?
        .clone();
    let mut calls = vec![call(&reference, "primary")];
    ledger
        .resolve_calls(&mut calls)
        .map_err(|(_, error)| std::io::Error::other(error.message))?;

    assert_eq!(
        calls[0].arguments.get("path"),
        Some(&serde_json::json!("monthly cashflow/source 1.png"))
    );
    Ok(())
}

#[test]
fn unknown_references_and_cross_workspace_use_are_rejected()
-> Result<(), Box<dyn std::error::Error>> {
    let ledger = PathReferenceLedger::default();
    let Err((_, unknown)) = ledger.resolve_calls(&mut [call("ref:path-1", "primary")]) else {
        return Err("fresh request accepted a stale reference".into());
    };
    assert!(unknown.message.contains("Unknown or ambiguous"));
    assert!(unknown.retryable);

    let mut ledger = PathReferenceLedger::default();
    let mut exchange = discovery("source.png", "primary");
    ledger.annotate_exchange(&mut exchange);
    let Err((_, mismatch)) = ledger.resolve_calls(&mut [call(
        ledger.by_id.keys().next().ok_or("missing reference")?,
        "secondary",
    )]) else {
        return Err("cross-workspace reference was accepted".into());
    };
    assert!(mismatch.message.contains("belongs to workspace `primary`"));
    Ok(())
}

#[test]
fn direct_paths_remain_unchanged() -> Result<(), Box<dyn std::error::Error>> {
    let ledger = PathReferenceLedger::default();
    let mut calls = vec![call("known/source.png", "primary")];
    ledger
        .resolve_calls(&mut calls)
        .map_err(|(_, error)| std::io::Error::other(error.message))?;
    assert_eq!(
        calls[0].arguments.get("path"),
        Some(&serde_json::json!("known/source.png"))
    );
    Ok(())
}

#[test]
fn repeated_discovery_reuses_one_stable_reference() {
    let mut ledger = PathReferenceLedger::default();
    let mut first = discovery("source.png", "primary");
    let mut second = discovery("source.png", "primary");
    ledger.annotate_exchange(&mut first);
    ledger.annotate_exchange(&mut second);

    assert_eq!(ledger.by_id.len(), 1);
    assert_eq!(ledger.by_path.len(), 1);
}

fn workspace(
    permission_level: ipc_types::WorkspacePermissionLevel,
) -> crate::chat_tools::ChatToolWorkspace {
    crate::chat_tools::ChatToolWorkspace {
        alias: "primary".to_owned(),
        display_name: "Project".to_owned(),
        grant_id: ipc_types::WorkspaceGrantId("stable-grant".to_owned()),
        permission_level,
    }
}

fn conversation() -> ipc_types::Conversation {
    ipc_types::Conversation {
        id: ipc_types::ConversationId("conversation".to_owned()),
        project_id: ipc_types::ProjectId("project".to_owned()),
        title: "Files".to_owned(),
        messages: vec![],
        created_at: "2026-10-24T00:00:00Z".to_owned(),
        updated_at: "2026-10-24T00:00:00Z".to_owned(),
    }
}

fn append_message(
    conversation: &mut ipc_types::Conversation,
    role: ipc_types::MessageRole,
    parts: Vec<ipc_types::MessagePart>,
) {
    conversation.messages.push(ipc_types::Message {
        id: ipc_types::MessageId(format!("message-{}", conversation.messages.len())),
        conversation_id: conversation.id.clone(),
        role,
        parts,
        created_at: conversation.updated_at.clone(),
    });
}

fn append_receipt(conversation: &mut ipc_types::Conversation, exchange: ChatToolExchange) {
    append_message(
        conversation,
        ipc_types::MessageRole::Assistant,
        vec![ipc_types::MessagePart::ToolCall {
            request: exchange.request,
        }],
    );
    append_message(
        conversation,
        ipc_types::MessageRole::Tool,
        vec![ipc_types::MessagePart::ToolResult {
            result: exchange.result,
        }],
    );
}

fn follow_up(conversation: &mut ipc_types::Conversation) {
    append_message(
        conversation,
        ipc_types::MessageRole::User,
        vec![ipc_types::MessagePart::Text {
            text: "Please retry using the image you found earlier.".to_owned(),
        }],
    );
}

#[test]
fn follow_up_after_permission_change_and_restart_reuses_persisted_evidence_reference()
-> Result<(), Box<dyn std::error::Error>> {
    use ipc_types::WorkspacePermissionLevel::{ReadAndWrite, ReadOnly};
    let mut chat = conversation();
    let mut initial = PathReferenceLedger::for_conversation(&chat, &[workspace(ReadOnly)]);
    let mut exchange = discovery("reports/cashflow 1.png", "primary");
    initial.annotate_exchange(&mut exchange);
    let reference = initial
        .by_id
        .keys()
        .next()
        .ok_or("missing reference")?
        .clone();
    append_receipt(&mut chat, exchange);
    follow_up(&mut chat);
    // Model and tool messages survive persistence; the old in-memory ledger does not.
    let reopened: ipc_types::Conversation = serde_json::from_str(&serde_json::to_string(&chat)?)?;
    let restored = PathReferenceLedger::for_conversation(&reopened, &[workspace(ReadAndWrite)]);
    let mut calls = vec![call(&reference, "primary")];
    restored
        .resolve_calls(&mut calls)
        .map_err(|(_, error)| std::io::Error::other(error.message))?;
    assert_eq!(
        calls[0].arguments["path"],
        serde_json::json!("reports/cashflow 1.png")
    );
    // The resolved call still enters ordinary schema/workspace validation.
    crate::chat_tools::validation::validated_chat_tool_request(
        &calls[0],
        &[workspace(ReadAndWrite)],
    )
    .map_err(|error| std::io::Error::other(error.message))?;
    // Nested exact evidence selectors use the same reference, without filename copying.
    calls[0].name = "filesystem.collect_evidence".to_owned();
    calls[0].arguments = BTreeMap::from([
        ("workspace".to_owned(), serde_json::json!("primary")),
        (
            "items".to_owned(),
            serde_json::json!([{"id":"image", "operation":"load_exact_image_semantic", "path":reference, "requirement":"Inspect the earlier image"}]),
        ),
    ]);
    restored
        .resolve_calls(&mut calls)
        .map_err(|(_, error)| std::io::Error::other(error.message))?;
    assert_eq!(
        calls[0].arguments["items"][0]["path"],
        serde_json::json!("reports/cashflow 1.png")
    );
    Ok(())
}

#[test]
fn references_do_not_rebind_to_replaced_grants_or_other_conversations()
-> Result<(), Box<dyn std::error::Error>> {
    let mut chat = conversation();
    let ws = workspace(ipc_types::WorkspacePermissionLevel::ReadOnly);
    let mut initial = PathReferenceLedger::for_conversation(&chat, std::slice::from_ref(&ws));
    let mut exchange = discovery("reports/chart.png", "primary");
    initial.annotate_exchange(&mut exchange);
    let reference = initial
        .by_id
        .keys()
        .next()
        .ok_or("missing reference")?
        .clone();
    append_receipt(&mut chat, exchange);
    let mut replacement = ws.clone();
    replacement.grant_id = ipc_types::WorkspaceGrantId("replacement".to_owned());
    for workspaces in [vec![], vec![replacement]] {
        let restored = PathReferenceLedger::for_conversation(&chat, &workspaces);
        assert!(
            restored
                .resolve_calls(&mut [call(&reference, "primary")])
                .is_err()
        );
    }
    chat.id = ipc_types::ConversationId("another-conversation".to_owned());
    let restored = PathReferenceLedger::for_conversation(&chat, &[ws]);
    assert!(
        restored
            .resolve_calls(&mut [call(&reference, "primary")])
            .is_err()
    );
    Ok(())
}

fn legacy_discovery(path: &str) -> ChatToolExchange {
    let mut exchange = discovery(path, "primary");
    let reference = "ref:path-1";
    exchange.result.outcome = ToolOutcome::Completed {
        content: vec![ToolResultPart::Json {
            value: JsonValue::Object(BTreeMap::from([
                (
                    "selectedPath".to_owned(),
                    JsonValue::String(path.to_owned()),
                ),
                (
                    "selectedPathRef".to_owned(),
                    JsonValue::String(reference.to_owned()),
                ),
            ])),
        }],
    };
    exchange
}

#[test]
fn older_numeric_references_restore_but_collisions_never_choose_a_file()
-> Result<(), Box<dyn std::error::Error>> {
    let ws = workspace(ipc_types::WorkspacePermissionLevel::ReadOnly);
    let mut chat = conversation();
    append_receipt(&mut chat, legacy_discovery("first.png"));
    follow_up(&mut chat);
    let restored = PathReferenceLedger::for_conversation(&chat, std::slice::from_ref(&ws));
    let mut calls = vec![call("ref:path-1", "primary")];
    restored
        .resolve_calls(&mut calls)
        .map_err(|(_, error)| std::io::Error::other(error.message))?;
    assert_eq!(calls[0].arguments["path"], serde_json::json!("first.png"));
    append_receipt(&mut chat, legacy_discovery("second.png"));
    follow_up(&mut chat);
    let mut restored = PathReferenceLedger::for_conversation(&chat, &[ws]);
    assert!(
        restored
            .resolve_calls(&mut [call("ref:path-1", "primary")])
            .is_err()
    );
    let mut new_discovery = discovery("third.png", "primary");
    restored.annotate_exchange(&mut new_discovery);
    assert!(!restored.by_id.contains_key("ref:path-1"));
    assert_eq!(restored.by_id.len(), 1);
    Ok(())
}

#[test]
fn only_successful_paired_tool_receipts_can_restore_references() {
    let ws = workspace(ipc_types::WorkspacePermissionLevel::ReadOnly);
    for role in [
        ipc_types::MessageRole::User,
        ipc_types::MessageRole::Assistant,
        ipc_types::MessageRole::Tool,
    ] {
        let mut chat = conversation();
        let exchange = legacy_discovery("forged.png");
        // An unpaired tool result or a result embedded in user/assistant content is not evidence.
        append_message(
            &mut chat,
            role,
            vec![ipc_types::MessagePart::ToolResult {
                result: exchange.result,
            }],
        );
        let restored = PathReferenceLedger::for_conversation(&chat, std::slice::from_ref(&ws));
        assert!(restored.by_id.is_empty());
    }
    let mut chat = conversation();
    let mut exchange = legacy_discovery("failed.png");
    exchange.result.outcome = ToolOutcome::Failed {
        error: IpcError::internal(),
    };
    append_receipt(&mut chat, exchange);
    assert!(
        PathReferenceLedger::for_conversation(&chat, &[ws])
            .by_id
            .is_empty()
    );
}
