use super::*;
use ipc_types::{TaskActivityKind, TaskActivityStatus, ToolCallId, ToolRequest, ToolResult};
use std::collections::BTreeMap;

fn pass(request: &str, round: usize) -> TaskActivity {
    TaskActivity {
        id: format!("model-pass-{request}-{round}"),
        kind: TaskActivityKind::ModelPass,
        title: format!("Model pass {}", round + 1),
        detail: Some("Reading 2s".into()),
        status: TaskActivityStatus::Completed,
    }
}
fn exchange(id: &str) -> ChatToolExchange {
    ChatToolExchange {
        request: ToolRequest {
            call_id: ToolCallId(id.into()),
            name: ToolName::FilesystemList,
            arguments: BTreeMap::default(),
        },
        result: ToolResult {
            call_id: ToolCallId(id.into()),
            outcome: ToolOutcome::Completed { content: vec![] },
        },
        activity: TaskActivity {
            id: id.into(),
            kind: TaskActivityKind::FileRead,
            title: "Read directory".into(),
            detail: None,
            status: TaskActivityStatus::Completed,
        },
        workspace_grant_id: None,
        undo_changes: vec![],
        inspected_asset_id: None,
        computation_reuse_digest: None,
    }
}
fn activities(conversation: &Conversation) -> Vec<String> {
    conversation
        .messages
        .iter()
        .flat_map(|message| &message.parts)
        .filter_map(|part| match part {
            MessagePart::Activity { activity } => Some(activity.id.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn reloaded_history_keeps_each_pass_before_its_work_and_final_pass_only_once()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let core = AppCore::initialize(directory.path())?;
    let date = "2026-11-13T00:00:00Z";
    let project = core.create_project("Synthetic chronology", date)?;
    let mut conversation = core.create_project_conversation(&project.id, date)?;
    let request = RequestId("request-a".into());
    let first = pass("request-a", 0);
    let second = pass("request-a", 1);
    persist_tool_exchanges(
        &core,
        &request,
        &mut conversation,
        std::slice::from_ref(&first),
        String::new(),
        String::new(),
        vec![Ok(exchange("read-a"))],
        &[],
        "",
    )
    .map_err(|error| error.message)?;
    persist_tool_exchanges(
        &core,
        &request,
        &mut conversation,
        &[first.clone(), second.clone()],
        String::new(),
        String::new(),
        vec![Ok(exchange("read-b"))],
        &[],
        "",
    )
    .map_err(|error| error.message)?;
    core.append_assistant_message(
        &conversation.id,
        Some("Synthetic final reasoning".into()),
        vec![first, second, pass("request-a", 2)],
        "Finished".into(),
        date,
    )?;
    let restored = core
        .conversation(&conversation.id)?
        .ok_or("missing conversation")?;
    assert_eq!(
        activities(&restored),
        vec![
            "model-pass-request-a-0",
            "read-a",
            "model-pass-request-a-1",
            "read-b",
            "model-pass-request-a-2"
        ]
    );
    assert_eq!(
        restored.messages.last().map(|message| message.role.clone()),
        Some(MessageRole::Assistant)
    );
    assert!(
        matches!(restored.messages.last().and_then(|message|message.parts.first()),Some(MessagePart::Activity { activity }) if activity.id == "model-pass-request-a-2")
    );
    Ok(())
}

#[test]
fn interrupted_request_keeps_completed_passes_without_a_phantom_final_answer()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let core = AppCore::initialize(directory.path())?;
    let date = "2026-11-13T00:00:00Z";
    let project = core.create_project("Synthetic interrupted chronology", date)?;
    let mut conversation = core.create_project_conversation(&project.id, date)?;
    let request = RequestId("interrupted".into());
    persist_tool_exchanges(
        &core,
        &request,
        &mut conversation,
        &[pass("interrupted", 0)],
        String::new(),
        String::new(),
        vec![Ok(exchange("already-read"))],
        &[],
        "",
    )
    .map_err(|error| error.message)?;
    let restored = core
        .conversation(&conversation.id)?
        .ok_or("missing conversation")?;
    assert_eq!(
        activities(&restored),
        vec!["model-pass-interrupted-0", "already-read"]
    );
    assert!(
        restored
            .messages
            .iter()
            .filter(|message| message.role == MessageRole::Assistant)
            .all(|message| message
                .parts
                .iter()
                .any(|part| matches!(part, MessagePart::ToolCall { .. })))
    );
    Ok(())
}
