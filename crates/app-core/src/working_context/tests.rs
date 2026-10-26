use std::{collections::BTreeMap, error::Error};

use ipc_types::{
    ConversationId, IpcError, MessageId, ProjectId, ToolCallId, ToolName, ToolRequest, ToolResult,
    ToolResultPart,
};
use tempfile::TempDir;

use super::*;
use crate::{AppCore, Project};
use agent_core::{
    ExecutionBudgets, ExecutionLedger, LedgerNextAction, LedgerRequest,
    build_agent_system_instructions,
};

const NOW: &str = "2026-08-18T10:00:00Z";

fn message(
    conversation_id: &ConversationId,
    id: &str,
    role: MessageRole,
    parts: Vec<MessagePart>,
) -> Message {
    Message {
        id: MessageId(id.to_owned()),
        conversation_id: conversation_id.clone(),
        role,
        parts,
        created_at: NOW.to_owned(),
    }
}

fn text_message(
    conversation_id: &ConversationId,
    id: &str,
    role: MessageRole,
    text: String,
) -> Message {
    message(conversation_id, id, role, vec![MessagePart::Text { text }])
}

fn long_conversation() -> Conversation {
    let id = ConversationId("context-conversation".to_owned());
    Conversation {
        id: id.clone(),
        project_id: ProjectId("project-default".to_owned()),
        title: "Long task".to_owned(),
        messages: vec![
            text_message(
                &id,
                "request",
                MessageRole::User,
                format!("ORIGINAL_REQUEST update the parser {}", "a".repeat(5_000)),
            ),
            text_message(
                &id,
                "decision",
                MessageRole::Assistant,
                format!("DECISION keep compatibility {}", "b".repeat(5_000)),
            ),
            text_message(
                &id,
                "constraint",
                MessageRole::User,
                format!("CONSTRAINT do not change the API {}", "c".repeat(5_000)),
            ),
            text_message(
                &id,
                "answer",
                MessageRole::Assistant,
                format!("Acknowledged constraint {}", "d".repeat(5_000)),
            ),
            text_message(
                &id,
                "latest",
                MessageRole::User,
                "LATEST_FOLLOW_UP include tests".to_owned(),
            ),
        ],
        created_at: NOW.to_owned(),
        updated_at: NOW.to_owned(),
    }
}

#[test]
fn compaction_preserves_semantics_and_keeps_recent_follow_up_verbatim() -> Result<(), Box<dyn Error>>
{
    let conversation = long_conversation();
    let original = conversation.clone();
    let working = derive(&conversation)?;
    assert_eq!(working.message_range(), 4..5);
    let summary = working
        .fallback_summary()
        .ok_or("summary missing")?
        .render_for_prompt()?;
    assert!(summary.contains("trust=\"untrusted_recap\""));
    assert!(summary.contains("ORIGINAL_REQUEST update the parser"));
    assert!(summary.contains("CONSTRAINT do not change the API"));
    assert!(summary.contains("DECISION keep compatibility"));
    assert!(!summary.contains(&"b".repeat(1_000)));
    assert_eq!(conversation, original);
    assert!(matches!(
        &conversation.messages[working.message_range()][0].parts[0],
        MessagePart::Text { text } if text == "LATEST_FOLLOW_UP include tests"
    ));
    Ok(())
}

#[test]
fn repeated_compaction_advances_to_the_newest_complete_follow_up() -> Result<(), Box<dyn Error>> {
    let mut conversation = long_conversation();
    let first = derive(&conversation)?;
    assert_eq!(first.message_range(), 4..5);
    conversation.messages.push(text_message(
        &conversation.id,
        "latest-answer",
        MessageRole::Assistant,
        "The tests are included.".to_owned(),
    ));
    conversation.messages.push(text_message(
        &conversation.id,
        "newest-follow-up",
        MessageRole::User,
        "NEWEST_FOLLOW_UP now explain the failure".to_owned(),
    ));
    let repeated = derive(&conversation)?;
    assert_eq!(repeated.message_range(), 4..7);
    let summary = repeated
        .fallback_summary()
        .ok_or("repeated summary missing")?
        .render_for_prompt()?;
    assert!(summary.contains("ORIGINAL_REQUEST update the parser"));
    assert!(matches!(
        &conversation.messages[repeated.message_range()][0].parts[0],
        MessagePart::Text { text } if text == "LATEST_FOLLOW_UP include tests"
    ));
    assert!(matches!(
        &conversation.messages[repeated.message_range()]
            .last()
            .ok_or("newest follow-up missing")?
            .parts[0],
        MessagePart::Text { text } if text == "NEWEST_FOLLOW_UP now explain the failure"
    ));
    Ok(())
}

#[test]
fn recent_tool_call_and_mutation_outcome_remain_one_verbatim_round() -> Result<(), Box<dyn Error>> {
    let mut conversation = long_conversation();
    let latest = conversation.messages.pop().ok_or("latest turn missing")?;
    conversation.messages.push(latest);
    let call_id = ToolCallId("write-call".to_owned());
    conversation.messages.push(message(
        &conversation.id,
        "write-request",
        MessageRole::Assistant,
        vec![MessagePart::ToolCall {
            request: ToolRequest {
                call_id: call_id.clone(),
                name: ToolName::FilesystemReplaceText,
                arguments: BTreeMap::from([
                    (
                        "workspace".to_owned(),
                        JsonValue::String("primary".to_owned()),
                    ),
                    (
                        "path".to_owned(),
                        JsonValue::String("src/lib.rs".to_owned()),
                    ),
                ]),
            },
        }],
    ));
    conversation.messages.push(message(
        &conversation.id,
        "write-result",
        MessageRole::Tool,
        vec![MessagePart::ToolResult {
            result: ToolResult {
                call_id,
                outcome: ToolOutcome::Completed {
                    content: vec![ToolResultPart::Text {
                        text: "mutation applied".to_owned(),
                    }],
                },
            },
        }],
    ));

    let working = derive(&conversation)?;
    assert_eq!(working.message_range(), 4..7);
    assert_eq!(conversation.messages[5].role, MessageRole::Assistant);
    assert_eq!(conversation.messages[6].role, MessageRole::Tool);
    Ok(())
}

#[test]
fn oversized_active_tool_round_defers_to_exact_prompt_admission() -> Result<(), Box<dyn Error>> {
    let id = ConversationId("active-tool-round".to_owned());
    let conversation = Conversation {
        id: id.clone(),
        project_id: ProjectId("project-default".to_owned()),
        title: "Reconstruct table".to_owned(),
        messages: vec![
            text_message(
                &id,
                "request",
                MessageRole::User,
                "Convert the monthly cashflow image".to_owned(),
            ),
            text_message(
                &id,
                "directory-results",
                MessageRole::Assistant,
                "a".repeat(9_000),
            ),
            text_message(&id, "manifest-result", MessageRole::Tool, "b".repeat(9_000)),
        ],
        created_at: NOW.to_owned(),
        updated_at: NOW.to_owned(),
    };

    let working = derive(&conversation)?;
    assert_eq!(working.message_range(), 0..3);
    assert!(working.compacted_message_range().is_none());
    assert!(working.fallback_summary().is_none());
    Ok(())
}

#[test]
fn older_rounds_compact_when_the_latest_round_exceeds_the_planning_estimate()
-> Result<(), Box<dyn Error>> {
    let mut conversation = long_conversation();
    let latest = conversation.messages.last_mut().ok_or("latest missing")?;
    latest.parts = vec![MessagePart::Text {
        text: format!("LATEST_FOLLOW_UP {}", "z".repeat(18_000)),
    }];

    let working = derive(&conversation)?;
    assert_eq!(working.message_range(), 4..5);
    assert_eq!(working.compacted_message_range(), Some(0..4));
    assert!(working.fallback_summary().is_some());
    Ok(())
}

#[test]
fn complete_timeline_survives_rendering_and_application_restart() -> Result<(), Box<dyn Error>> {
    let parent = TempDir::new()?;
    let data_directory = parent.path().join("Zeta");
    let conversation = long_conversation();
    let expected = conversation.clone();
    let prompt = {
        let app = AppCore::initialize(&data_directory)?;
        app.with_storage(|storage| {
            storage.save_project(&Project {
                id: conversation.project_id.clone(),
                name: "Test project".to_owned(),
                created_at: NOW.to_owned(),
                updated_at: NOW.to_owned(),
            })?;
            storage.save_conversation(&conversation)
        })?;
        let (prompt, assets) = app.render_chat_prompt(&conversation, "2026-08-18")?;
        assert!(assets.is_empty());
        assert!(prompt.contains("ORIGINAL_REQUEST update the parser"));
        assert!(prompt.contains("LATEST_FOLLOW_UP include tests"));
        assert_eq!(app.conversation(&conversation.id)?, Some(expected.clone()));
        prompt
    };

    let reopened = AppCore::initialize(&data_directory)?;
    let restored = reopened
        .conversation(&conversation.id)?
        .ok_or("conversation missing after restart")?;
    assert_eq!(restored, expected);
    let (restored_prompt, _) = reopened.render_chat_prompt(&restored, "2026-08-18")?;
    assert_eq!(restored_prompt, prompt);
    Ok(())
}

#[test]
fn final_prompt_merges_a_fresh_authoritative_ledger_projection() -> Result<(), Box<dyn Error>> {
    let parent = TempDir::new()?;
    let app = AppCore::initialize(parent.path().join("Zeta"))?;
    let conversation = long_conversation();
    app.with_storage(|storage| {
        storage.save_project(&Project {
            id: conversation.project_id.clone(),
            name: "Test project".to_owned(),
            created_at: NOW.to_owned(),
            updated_at: NOW.to_owned(),
        })?;
        storage.save_conversation(&conversation)
    })?;
    let ledger = ExecutionLedger::new(
        "run-authoritative",
        LedgerRequest::new(
            "request-authoritative",
            conversation.id.0.clone(),
            conversation.project_id.0.clone(),
            "AUTHORITATIVE_LEDGER_REQUEST",
        )?,
        build_agent_system_instructions("2026-08-18", None)?
            .identity()
            .clone(),
        ExecutionBudgets {
            rounds: 1,
            tool_calls: 0,
            elapsed_ms: 30_000,
            tool_ms: 0,
            context_tokens: 32_768,
            final_response_tokens: 1_024,
            tool_output_bytes: 0,
        },
        LedgerNextAction::Generate,
    )?;
    let snapshot = ledger.snapshot()?;
    app.with_storage(|storage| storage.save_execution_ledger(&snapshot, NOW))?;

    let (prompt, _) = app.render_chat_prompt(&conversation, "2026-08-18")?;
    assert!(prompt.contains("<authoritative_execution_ledger trust=\"host\">"));
    assert!(prompt.contains("AUTHORITATIVE_LEDGER_REQUEST"));
    // Preview preserves exact dialogue; only failed exact admission can compact.
    assert!(!prompt.contains("trust=\"untrusted_recap\""));
    assert!(prompt.contains("LATEST_FOLLOW_UP include tests"));
    Ok(())
}

#[test]
fn failed_tool_outcomes_are_preserved_as_untrusted_unresolved_work() -> Result<(), Box<dyn Error>> {
    let mut conversation = long_conversation();
    let id = conversation.id.clone();
    conversation.messages.insert(
        2,
        message(
            &id,
            "old-failure",
            MessageRole::Tool,
            vec![MessagePart::ToolResult {
                result: ToolResult {
                    call_id: ToolCallId("old-call".to_owned()),
                    outcome: ToolOutcome::Failed {
                        error: IpcError::invalid_request("source revision changed"),
                    },
                },
            }],
        ),
    );
    let summary = derive(&conversation)?
        .fallback_summary()
        .ok_or("failure summary missing")?
        .render_for_prompt()?;
    assert!(summary.contains("source revision changed"));
    assert!(summary.contains("untrusted_recap"));
    Ok(())
}
