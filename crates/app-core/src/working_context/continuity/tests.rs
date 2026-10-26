use super::*;
use crate::Project;
use crate::working_context::derive;
use ipc_types::{ConversationId, Message, MessageId, MessagePart, ProjectId};
const NOW: &str = "2026-09-10T12:00:00Z";
fn conversation() -> Conversation {
    Conversation {
        id: ConversationId("chat".into()),
        project_id: ProjectId("project".into()),
        title: "Task".into(),
        messages: vec![
            message("request", MessageRole::User, "Create the report"),
            message("question", MessageRole::Assistant, "Which account?"),
            message("answer", MessageRole::User, "Account A"),
        ],
        created_at: NOW.into(),
        updated_at: NOW.into(),
    }
}
fn message(id: &str, role: MessageRole, text: &str) -> Message {
    Message {
        id: MessageId(id.into()),
        conversation_id: ConversationId("chat".into()),
        role,
        parts: vec![MessagePart::Text { text: text.into() }],
        created_at: NOW.into(),
    }
}
fn save(core: &AppCore, conversation: &Conversation) -> Result<(), AppCoreError> {
    core.with_storage(|store| {
        store.save_project(&Project {
            id: conversation.project_id.clone(),
            name: "Project".into(),
            created_at: NOW.into(),
            updated_at: NOW.into(),
        })?;
        store.save_conversation(conversation)
    })
}
#[test]
fn replies_revisions_and_reopen_do_not_advance_dialogue() -> Result<(), Box<dyn std::error::Error>>
{
    let dir = tempfile::TempDir::new()?;
    let path = dir.path().join("Zeta");
    let mut conversation = conversation();
    {
        let core = AppCore::initialize(&path)?;
        save(&core, &conversation)?;
        assert_eq!(prepare(&core, &conversation)?.message_range(), 0..3);
        conversation.messages.push(message(
            "plan",
            MessageRole::Assistant,
            &"Plan revision ".repeat(3000),
        ));
        conversation
            .messages
            .push(message("reply", MessageRole::User, "Use the account above"));
        core.with_storage(|store| store.save_conversation(&conversation))?;
        assert!(derive(&conversation)?.message_range().start > 0);
        assert_eq!(prepare(&core, &conversation)?.message_range(), 0..5);
    }
    let core = AppCore::initialize(&path)?;
    let restored = core
        .conversation(&conversation.id)?
        .ok_or("missing conversation")?;
    assert_eq!(preview(&core, &restored)?.message_range(), 0..5);
    assert_eq!(prepare(&core, &restored)?.message_range(), 0..5);
    Ok(())
}
#[test]
fn first_preparation_preserves_exact_dialogue_until_explicit_pressure()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::TempDir::new()?;
    let core = AppCore::initialize(dir.path().join("Zeta"))?;
    let mut conversation = conversation();
    conversation.messages[0] = message("request", MessageRole::User, &"Older task ".repeat(3000));
    save(&core, &conversation)?;
    assert!(derive(&conversation)?.message_range().start > 0);
    let expected = 0..conversation.messages.len();
    let working = prepare(&core, &conversation)?;
    assert_eq!(working.message_range(), expected);
    assert_eq!(working.compacted_message_range(), None);
    assert!(working.fallback_summary().is_none());
    conversation
        .messages
        .push(message("followup", MessageRole::User, "Keep these inputs"));
    assert_eq!(
        prepare(&core, &conversation)?.message_range().start,
        expected.start
    );
    conversation.messages[0] = message("request", MessageRole::User, "tampered covered history");
    assert!(prepare(&core, &conversation).is_err());
    Ok(())
}
#[test]
fn changed_or_missing_anchor_is_not_silently_reconstructed()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::TempDir::new()?;
    let core = AppCore::initialize(dir.path().join("Zeta"))?;
    let mut conversation = conversation();
    save(&core, &conversation)?;
    prepare(&core, &conversation)?;
    conversation.messages[0].parts = vec![MessagePart::Text {
        text: "changed".into(),
    }];
    assert!(prepare(&core, &conversation).is_err());
    conversation.messages.clear();
    assert!(prepare(&core, &conversation).is_err());
    Ok(())
}

#[test]
fn pressure_is_explicit_forward_only_and_persistent() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::TempDir::new()?;
    let core = AppCore::initialize(dir.path().join("Zeta"))?;
    let conversation = conversation();
    save(&core, &conversation)?;
    let prior = prepare(&core, &conversation)?;
    let next = pressure_candidate(&conversation, &prior)?;
    assert_eq!(next.message_range(), 2..3);
    assert_eq!(preview(&core, &conversation)?.message_range(), 0..3);
    commit_pressure(&core, &conversation, &prior, &next)?;
    assert_eq!(prepare(&core, &conversation)?.message_range(), 2..3);
    assert!(commit_pressure(&core, &conversation, &prior, &next).is_err());
    assert!(pressure_candidate(&conversation, &next).is_err());
    Ok(())
}

#[test]
fn pressure_never_splits_an_unfinished_native_group() -> Result<(), Box<dyn std::error::Error>> {
    let mut conversation = conversation();
    conversation.messages[1].parts.push(MessagePart::ToolCall {
        request: ipc_types::ToolRequest {
            call_id: ipc_types::ToolCallId("pending".into()),
            name: ipc_types::ToolName::WebSearch,
            arguments: std::collections::BTreeMap::new(),
        },
    });
    let working = derive(&conversation)?;
    assert!(matches!(
        pressure_candidate(&conversation, &working),
        Err(AppCoreError::WorkingContext(
            WorkingContextError::ContextCapacityGap
        ))
    ));
    Ok(())
}
