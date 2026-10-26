//! Chronological dialogue and paired evidence selection for inference.

mod pressure;
use model_runtime::PromptMessage;
use pressure::pressure_projection;

use super::{
    AssetId, BTreeSet, Conversation, JsonValue, Message, MessagePart, MessageRole, ToolOutcome,
    ToolResultPart, tool_name,
};

pub(super) fn selected_bounded_history(
    conversation: &Conversation,
    range: std::ops::Range<usize>,
    focus: Option<&BTreeSet<String>>,
    compact_completed_exchanges: bool,
    preserve_native_reasoning: bool,
) -> (Vec<Message>, usize) {
    let history = selected_history(conversation, range, focus, preserve_native_reasoning);
    let original_count = history.len();
    let history = if compact_completed_exchanges {
        pressure_projection(history)
    } else {
        history
    };
    let omitted = original_count.saturating_sub(history.len());
    (history, omitted)
}

pub(super) fn selected_tool_names(history: &[PromptMessage]) -> Vec<String> {
    history
        .iter()
        .filter_map(|message| match message {
            PromptMessage::Assistant { tool_calls, .. } => Some(tool_calls),
            _ => None,
        })
        .flatten()
        .map(|call| call.name.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub(super) fn selected_prompt_history(
    conversation: &Conversation,
    range: std::ops::Range<usize>,
    focus: Option<&BTreeSet<String>>,
) -> Vec<Message> {
    selected_history(conversation, range, focus, false)
}

fn selected_history(
    conversation: &Conversation,
    range: std::ops::Range<usize>,
    focus: Option<&BTreeSet<String>>,
    preserve_native_reasoning: bool,
) -> Vec<Message> {
    if let Some(call_ids) = focus {
        // Stages may add older evidence to the prompt, but cannot remove paired
        // exchanges from retained dialogue. In particular, a standalone accepted
        // plan must remain visible after an earlier task was deferred for planning.
        let mut retained = call_ids.clone();
        retained.extend(
            conversation.messages[range.clone()]
                .iter()
                .flat_map(|message| &message.parts)
                .filter_map(|part| match part {
                    MessagePart::ToolCall { request } => Some(request.call_id.0.clone()),
                    _ => None,
                }),
        );
        return focused_history(conversation, &retained, range, preserve_native_reasoning);
    }
    let mut messages = conversation.messages[..range.start]
        .iter()
        .filter_map(|message| {
            let parts = message
                .parts
                .iter()
                .filter(|part| matches!(part, MessagePart::Asset { .. }))
                .cloned()
                .collect::<Vec<_>>();
            (!parts.is_empty()).then(|| {
                let mut retained = message.clone();
                retained.parts = parts;
                retained
            })
        })
        .collect::<Vec<_>>();
    messages.extend_from_slice(&conversation.messages[range]);
    let call_names = history_call_names(conversation);
    let latest_user = messages
        .iter()
        .rposition(|message| message.role == MessageRole::User);
    for (index, message) in messages.iter_mut().enumerate() {
        for part in &mut message.parts {
            if matches!(part, MessagePart::ToolResult { .. }) {
                *part = compact_focused_result(part, &call_names);
            }
        }
        let native_continuation = latest_user.is_some_and(|user| index > user)
            && message
                .parts
                .iter()
                .any(|part| matches!(part, MessagePart::ToolCall { .. }));
        message.parts.retain(|part| {
            !matches!(part, MessagePart::Reasoning { .. })
                || native_continuation
                || preserve_native_reasoning
        });
    }
    messages.retain(|message| !message.parts.is_empty());
    messages
}

#[cfg(test)]
pub(crate) fn focused_phase_messages(
    conversation: &Conversation,
    call_ids: &BTreeSet<String>,
    range: std::ops::Range<usize>,
) -> Vec<Message> {
    focused_history(conversation, call_ids, range, false)
}

fn focused_history(
    conversation: &Conversation,
    call_ids: &BTreeSet<String>,
    range: std::ops::Range<usize>,
    preserve_native_reasoning: bool,
) -> Vec<Message> {
    let mut projected = Vec::new();
    let call_names = history_call_names(conversation);
    let selected_call_ids = focused_call_ids(conversation, call_ids);
    let latest_user = conversation
        .messages
        .iter()
        .rposition(|message| message.role == MessageRole::User);
    for (index, message) in conversation.messages.iter().enumerate() {
        let parts = match message.role {
            MessageRole::Assistant => message
                .parts
                .iter()
                .filter(|part| {
                    matches!(part, MessagePart::ToolCall { request } if selected_call_ids.contains(request.call_id.0.as_str()))
                        || (range.contains(&index) && matches!(part, MessagePart::Text { .. }))
                        || (preserve_native_reasoning && range.contains(&index)
                            && matches!(part, MessagePart::Reasoning { .. })
                            && message.parts.iter().all(|part| match part {
                                MessagePart::ToolCall { request } => selected_call_ids.contains(&request.call_id.0),
                                _ => true,
                            }))
                        || (latest_user.is_some_and(|user| index > user)
                            && message.parts.iter().any(|part| matches!(part, MessagePart::ToolCall { request } if selected_call_ids.contains(&request.call_id.0)))
                            && message.parts.iter().all(|part| match part {
                                MessagePart::ToolCall { request } => selected_call_ids.contains(&request.call_id.0),
                                _ => true,
                            })
                            && matches!(part, MessagePart::Reasoning { .. }))
                })
                .cloned()
                .collect::<Vec<_>>(),
            MessageRole::Tool => message
                .parts
                .iter()
                .filter(|part| {
                    matches!(part, MessagePart::ToolResult { result } if selected_call_ids.contains(result.call_id.0.as_str()))
                })
                .map(|part| compact_focused_result(part, &call_names))
                .collect::<Vec<_>>(),
            MessageRole::User if range.contains(&index) => message.parts.clone(),
            MessageRole::User => message.parts.iter().filter(|part| matches!(part, MessagePart::Asset { .. })).cloned().collect(),
        };
        if !parts.is_empty() {
            let mut message = message.clone();
            message.parts = parts;
            projected.push(message);
        }
    }
    projected
}

fn history_call_names(conversation: &Conversation) -> std::collections::BTreeMap<String, &str> {
    conversation
        .messages
        .iter()
        .flat_map(|message| &message.parts)
        .filter_map(|part| match part {
            MessagePart::ToolCall { request } => {
                Some((request.call_id.0.clone(), tool_name(&request.name)))
            }
            _ => None,
        })
        .collect::<std::collections::BTreeMap<_, _>>()
}

fn focused_call_ids(conversation: &Conversation, call_ids: &BTreeSet<String>) -> BTreeSet<String> {
    let latest_image_result_index = conversation
        .messages
        .iter()
        .enumerate()
        .filter(|(_, message)| message.role == MessageRole::Tool)
        .filter(|(_, message)| {
            message.parts.iter().any(|part| {
                matches!(part,
                    MessagePart::ToolResult { result }
                        if call_ids.contains(result.call_id.0.as_str())
                            && matches!(&result.outcome, ToolOutcome::Completed { content }
                                if content.iter().any(|part| matches!(part, ToolResultPart::Asset { .. })))
                )
            })
        })
        .map(|(index, _)| index)
        .next_back();
    let continuation_assistant_index = latest_image_result_index.and_then(|result_index| {
        conversation
            .messages
            .iter()
            .enumerate()
            .skip(result_index.saturating_add(1))
            .filter(|(_, message)| message.role == MessageRole::Assistant)
            .map(|(index, _)| index)
            .next_back()
    });
    let mut selected_call_ids = call_ids.clone();
    if let Some(index) = continuation_assistant_index {
        selected_call_ids.extend(completed_continuation_call_ids(conversation, index));
    }
    let result_ids = conversation
        .messages
        .iter()
        .flat_map(|message| &message.parts)
        .filter_map(|part| match part {
            MessagePart::ToolResult { result } => Some(result.call_id.0.clone()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    // A selected native assistant batch stays whole, but only if it has all
    // paired results. Incomplete history is not repaired by fabricating results.
    for message in &conversation.messages {
        let ids = message
            .parts
            .iter()
            .filter_map(|part| match part {
                MessagePart::ToolCall { request } => Some(request.call_id.0.clone()),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        if !ids.is_disjoint(&selected_call_ids) && ids.is_subset(&result_ids) {
            selected_call_ids.extend(ids);
        }
    }
    selected_call_ids
}

pub(crate) fn message_asset_ids(messages: &[Message]) -> Vec<AssetId> {
    messages
        .iter()
        .flat_map(|message| &message.parts)
        .flat_map(|part| match part {
            MessagePart::Asset { asset_id } => vec![asset_id.clone()],
            MessagePart::ToolResult { result } => match &result.outcome {
                ToolOutcome::Completed { content } => content
                    .iter()
                    .filter_map(|part| match part {
                        ToolResultPart::Asset { asset_id } => Some(asset_id.clone()),
                        _ => None,
                    })
                    .collect(),
                ToolOutcome::Failed { .. } => Vec::new(),
            },
            _ => Vec::new(),
        })
        .collect()
}

fn completed_continuation_call_ids(
    conversation: &Conversation,
    assistant_index: usize,
) -> BTreeSet<String> {
    let result_call_ids = conversation
        .messages
        .iter()
        .flat_map(|message| &message.parts)
        .filter_map(|part| match part {
            MessagePart::ToolResult { result } => Some(result.call_id.0.as_str()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    conversation.messages[assistant_index]
        .parts
        .iter()
        .filter_map(|part| {
            let MessagePart::ToolCall { request } = part else {
                return None;
            };
            result_call_ids
                .contains(request.call_id.0.as_str())
                .then(|| request.call_id.0.clone())
        })
        .collect()
}

fn compact_focused_result(
    part: &MessagePart,
    call_names: &std::collections::BTreeMap<String, &str>,
) -> MessagePart {
    let MessagePart::ToolResult { result } = part else {
        return part.clone();
    };
    let Some(operation) = call_names.get(result.call_id.0.as_str()).copied() else {
        return part.clone();
    };
    let mut result = result.clone();
    let ToolOutcome::Completed { content } = &mut result.outcome else {
        return MessagePart::ToolResult { result };
    };
    for item in content {
        let ToolResultPart::Json { value } = item else {
            continue;
        };
        if operation == "filesystem.find_paths" {
            *value = compact_find_paths(value);
        } else if operation == "filesystem.collect_evidence" {
            *value = compact_collected_evidence(value);
        }
    }
    MessagePart::ToolResult { result }
}

fn compact_find_paths(value: &JsonValue) -> JsonValue {
    let JsonValue::Object(root) = value else {
        return value.clone();
    };
    let Some(JsonValue::Array(results)) = root.get("results") else {
        return value.clone();
    };
    JsonValue::Object(std::collections::BTreeMap::from([
        (
            "projection".to_owned(),
            JsonValue::String("compact_active_state_v1".to_owned()),
        ),
        (
            "results".to_owned(),
            JsonValue::Array(results.iter().map(compact_path_query_result).collect()),
        ),
    ]))
}

fn compact_path_query_result(value: &JsonValue) -> JsonValue {
    let JsonValue::Object(fields) = value else {
        return value.clone();
    };
    let mut compact = std::collections::BTreeMap::new();
    for name in ["complete", "excludedCount", "omittedCount", "truncated"] {
        if let Some(value) = fields.get(name) {
            compact.insert(name.to_owned(), value.clone());
        }
    }
    if let Some(JsonValue::Array(matches)) = fields.get("matches") {
        compact.insert(
            "matches".to_owned(),
            JsonValue::Array(matches.iter().map(compact_path_match).collect()),
        );
    }
    JsonValue::Object(compact)
}

fn compact_path_match(value: &JsonValue) -> JsonValue {
    let JsonValue::Object(fields) = value else {
        return value.clone();
    };
    JsonValue::Object(
        ["relativePath", "pathRef", "kind", "byteSize"]
            .into_iter()
            .filter_map(|name| {
                fields
                    .get(name)
                    .cloned()
                    .map(|value| (name.to_owned(), value))
            })
            .collect(),
    )
}

fn compact_collected_evidence(value: &JsonValue) -> JsonValue {
    let JsonValue::Object(root) = value else {
        return value.clone();
    };
    let Some(JsonValue::Array(items)) = root.get("items") else {
        return value.clone();
    };
    let mut compact = std::collections::BTreeMap::new();
    compact.insert(
        "projection".to_owned(),
        JsonValue::String("compact_active_state_v1".to_owned()),
    );
    for name in ["ready", "coverageComplete"] {
        if let Some(value) = root.get(name) {
            compact.insert(name.to_owned(), value.clone());
        }
    }
    compact.insert(
        "items".to_owned(),
        JsonValue::Array(items.iter().map(compact_evidence_item).collect()),
    );
    JsonValue::Object(compact)
}

fn compact_evidence_item(value: &JsonValue) -> JsonValue {
    let JsonValue::Object(fields) = value else {
        return value.clone();
    };
    let mut compact = [
        "id",
        "operation",
        "selectedPath",
        "selectedPathRef",
        "candidateCount",
        "status",
        "text",
        "image",
    ]
    .into_iter()
    .filter_map(|name| {
        fields
            .get(name)
            .cloned()
            .map(|value| (name.to_owned(), value))
    })
    .collect::<std::collections::BTreeMap<_, _>>();
    if let Some(discovery) = fields.get("discovery") {
        compact.insert("discovery".to_owned(), compact_path_query_result(discovery));
    }
    JsonValue::Object(compact)
}
#[cfg(test)]
mod compact_tests {
    use super::*;

    #[test]
    fn filename_discovery_projection_keeps_candidates_and_drops_traversal_noise() {
        let value = JsonValue::Object(std::collections::BTreeMap::from([(
            "results".to_owned(),
            JsonValue::Array(vec![JsonValue::Object(std::collections::BTreeMap::from([
                ("complete".to_owned(), JsonValue::Bool(true)),
                ("directoriesScanned".to_owned(), JsonValue::Number(412.0)),
                (
                    "query".to_owned(),
                    JsonValue::Object(std::collections::BTreeMap::from([(
                        "nameContains".to_owned(),
                        JsonValue::String("cashflow".to_owned()),
                    )])),
                ),
                (
                    "matches".to_owned(),
                    JsonValue::Array(vec![JsonValue::Object(std::collections::BTreeMap::from([
                        (
                            "relativePath".to_owned(),
                            JsonValue::String("inputs/cashflow.png".to_owned()),
                        ),
                        (
                            "pathRef".to_owned(),
                            JsonValue::String("ref:path-1".to_owned()),
                        ),
                        ("kind".to_owned(), JsonValue::String("file".to_owned())),
                        (
                            "name".to_owned(),
                            JsonValue::String("cashflow.png".to_owned()),
                        ),
                    ]))]),
                ),
            ]))]),
        )]));
        let compact = compact_find_paths(&value);
        let encoded = serde_json::to_string(&compact).unwrap_or_default();
        assert!(encoded.contains("inputs/cashflow.png"));
        assert!(encoded.contains("ref:path-1"));
        assert!(encoded.contains("compact_active_state_v1"));
        assert!(!encoded.contains("directoriesScanned"));
        assert!(!encoded.contains("nameContains"));
    }

    #[test]
    fn evidence_projection_preserves_loaded_text_but_removes_repeated_requirement() {
        let value = JsonValue::Object(std::collections::BTreeMap::from([
            ("ready".to_owned(), JsonValue::Bool(true)),
            (
                "items".to_owned(),
                JsonValue::Array(vec![JsonValue::Object(std::collections::BTreeMap::from([
                    ("id".to_owned(), JsonValue::String("convention".to_owned())),
                    (
                        "requirement".to_owned(),
                        JsonValue::String("Load naming rules".repeat(40)),
                    ),
                    (
                        "selectedPath".to_owned(),
                        JsonValue::String("manifest.csv".to_owned()),
                    ),
                    (
                        "selectedPathRef".to_owned(),
                        JsonValue::String("ref:path-2".to_owned()),
                    ),
                    (
                        "text".to_owned(),
                        JsonValue::Object(std::collections::BTreeMap::from([(
                            "text".to_owned(),
                            JsonValue::String("monthly/{date}.csv".to_owned()),
                        )])),
                    ),
                ]))]),
            ),
        ]));
        let compact = compact_collected_evidence(&value);
        let encoded = serde_json::to_string(&compact).unwrap_or_default();
        assert!(encoded.contains("monthly/{date}.csv"));
        assert!(encoded.contains("manifest.csv"));
        assert!(encoded.contains("ref:path-2"));
        assert!(!encoded.contains("Load naming rules"));
    }
}
