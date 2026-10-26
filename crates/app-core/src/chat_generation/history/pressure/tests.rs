use super::*;
use ipc_types::{
    AssetId, ConversationId, MessageId, ToolCallId, ToolName, ToolRequest, ToolResult,
};

fn message(role: MessageRole, parts: Vec<MessagePart>, index: usize) -> Message {
    Message {
        id: MessageId(format!("message-{index}")),
        conversation_id: ConversationId("pressure".into()),
        role,
        parts,
        created_at: "2026-09-11T00:00:00Z".into(),
    }
}

fn batch(history: &mut Vec<Message>, id: &str, name: ToolName, content: Vec<ToolResultPart>) {
    history.push(message(
        MessageRole::Assistant,
        vec![
            MessagePart::Reasoning {
                text: format!("native continuation {id}"),
            },
            MessagePart::ToolCall {
                request: ToolRequest {
                    call_id: ToolCallId(id.into()),
                    name,
                    arguments: BTreeMap::new(),
                },
            },
        ],
        history.len(),
    ));
    history.push(message(
        MessageRole::Tool,
        vec![MessagePart::ToolResult {
            result: ToolResult {
                call_id: ToolCallId(id.into()),
                outcome: ToolOutcome::Completed { content },
            },
        }],
        history.len(),
    ));
}

fn text(value: &str) -> Vec<ToolResultPart> {
    vec![ToolResultPart::Text { text: value.into() }]
}

fn user_dialogue(items: &[Message]) -> Vec<&Message> {
    items
        .iter()
        .filter(|message| message.role == MessageRole::User)
        .collect()
}

#[test]
fn pressure_keeps_exact_replies_acceptance_recent_pairs_and_source_while_omitting_old_payloads() {
    let mut history = vec![message(
        MessageRole::User,
        vec![MessagePart::Text {
            text: "Choice A: navy; size L; initials ZX.\nKeep this exact.".into(),
        }],
        0,
    )];
    batch(
        &mut history,
        "old-read",
        ToolName::FilesystemReadText,
        text(&"old payload ".repeat(1000)),
    );
    history.push(message(
        MessageRole::User,
        vec![MessagePart::Text {
            text: "Choice B: size XL instead; recipient Rowan Example.".into(),
        }],
        history.len(),
    ));
    batch(
        &mut history,
        "accepted",
        ToolName::OrchestrationSubmitPlan,
        vec![ToolResultPart::Json {
            value: JsonValue::Object(BTreeMap::from([("accepted".into(), JsonValue::Bool(true))])),
        }],
    );
    batch(
        &mut history,
        "another-old-read",
        ToolName::FilesystemReadText,
        text("other old payload"),
    );
    batch(
        &mut history,
        "source",
        ToolName::FilesystemInspectImage,
        vec![ToolResultPart::Asset {
            asset_id: AssetId("exact-source".into()),
        }],
    );
    batch(
        &mut history,
        "old-after-image",
        ToolName::FilesystemReadText,
        text("old after image"),
    );
    batch(
        &mut history,
        "recent",
        ToolName::FilesystemReadText,
        text("recent fact"),
    );
    batch(
        &mut history,
        "consumer",
        ToolName::FilesystemWriteText,
        text("verified output receipt"),
    );
    let original = history.clone();
    let projected = pressure_projection(history.clone());
    assert_eq!(history, original);
    assert_eq!(user_dialogue(&projected), user_dialogue(&history));
    let calls = projected
        .iter()
        .flat_map(|m| &m.parts)
        .filter_map(|p| match p {
            MessagePart::ToolCall { request } => Some(request.call_id.0.as_str()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let results = projected
        .iter()
        .flat_map(|m| &m.parts)
        .filter_map(|p| match p {
            MessagePart::ToolResult { result } => Some(result.call_id.0.as_str()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        calls,
        BTreeSet::from(["accepted", "source", "recent", "consumer"])
    );
    assert_eq!(calls, results);
    for retained in &projected {
        assert_eq!(
            original.iter().find(|m| m.id == retained.id),
            Some(retained)
        );
    }
    assert!(
        serde_json::to_string(&projected).unwrap_or_default().len()
            < serde_json::to_string(&original).unwrap_or_default().len() / 2
    );
}

#[test]
fn pressure_does_not_split_a_native_batch_or_remove_an_unpaired_call() {
    let mut history = vec![];
    batch(
        &mut history,
        "image",
        ToolName::FilesystemInspectImage,
        vec![ToolResultPart::Asset {
            asset_id: AssetId("source".into()),
        }],
    );
    if let Some(assistant) = history.first_mut() {
        assistant.parts.push(MessagePart::ToolCall {
            request: ToolRequest {
                call_id: ToolCallId("sibling".into()),
                name: ToolName::FilesystemReadText,
                arguments: BTreeMap::new(),
            },
        });
    }
    history.push(message(
        MessageRole::Tool,
        vec![MessagePart::ToolResult {
            result: ToolResult {
                call_id: ToolCallId("sibling".into()),
                outcome: ToolOutcome::Completed {
                    content: text("sibling result"),
                },
            },
        }],
        history.len(),
    ));
    history.push(message(
        MessageRole::Assistant,
        vec![MessagePart::ToolCall {
            request: ToolRequest {
                call_id: ToolCallId("unpaired".into()),
                name: ToolName::FilesystemReadText,
                arguments: BTreeMap::new(),
            },
        }],
        history.len(),
    ));
    for id in ["one", "two", "three"] {
        batch(&mut history, id, ToolName::FilesystemReadText, text(id));
    }
    let projected = pressure_projection(history.clone());
    for expected in &history[..4] {
        assert_eq!(
            projected.iter().find(|m| m.id == expected.id),
            Some(expected)
        );
    }
}
