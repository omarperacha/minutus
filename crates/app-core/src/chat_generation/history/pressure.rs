//! Pressure-only omission of older completed exchanges, with exact scoped recall.

use std::collections::{BTreeMap, BTreeSet};

use ipc_types::{JsonValue, Message, MessagePart, MessageRole, ToolOutcome, ToolResultPart};

const RECENT_NATIVE_BATCHES: usize = 2;

/// Keep dialogue, the current accepted plan, recent native batches and the last
/// image producer. Omit only complete older batches; never orphan a native call.
/// The immutable conversation is untouched and supplies the recall manifest.
pub(in crate::chat_generation) fn pressure_projection(mut history: Vec<Message>) -> Vec<Message> {
    let batches = history
        .iter()
        .filter_map(|message| {
            let calls = message
                .parts
                .iter()
                .filter_map(|part| match part {
                    MessagePart::ToolCall { request } => Some(request.call_id.0.clone()),
                    _ => None,
                })
                .collect::<BTreeSet<_>>();
            (!calls.is_empty()).then_some(calls)
        })
        .collect::<Vec<_>>();
    let results = history
        .iter()
        .flat_map(|message| &message.parts)
        .filter_map(|part| match part {
            MessagePart::ToolResult { result } => Some(result.call_id.0.clone()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let mut keep = batches
        .iter()
        .rev()
        .take(RECENT_NATIVE_BATCHES)
        .flatten()
        .cloned()
        .collect::<BTreeSet<_>>();
    for batch in &batches {
        if !batch.is_subset(&results) {
            keep.extend(batch.iter().cloned());
        }
    }
    if let Some(id) = history.iter().rev().flat_map(|message| message.parts.iter().rev())
        .find_map(|part| match part {
            MessagePart::ToolResult { result } if matches!(&result.outcome,
                ToolOutcome::Completed { content } if content.iter().any(|item| matches!(item, ToolResultPart::Asset { .. }))) => Some(result.call_id.0.clone()),
            _ => None,
        }) {
        keep.insert(id);
    }
    retain_current_acceptance(&history, &mut keep);
    for batch in &batches {
        if !batch.is_disjoint(&keep) {
            keep.extend(batch.iter().cloned());
        }
    }
    for message in &mut history {
        match message.role {
            MessageRole::User => {}
            MessageRole::Assistant => {
                let retained_batch = message.parts.iter().any(|part| {
                    matches!(part,
                    MessagePart::ToolCall { request } if keep.contains(&request.call_id.0))
                });
                message.parts.retain(|part| match part {
                    MessagePart::Text { .. } => true,
                    MessagePart::Reasoning { .. } => retained_batch,
                    MessagePart::ToolCall { request } => keep.contains(&request.call_id.0),
                    _ => false,
                });
            }
            MessageRole::Tool => message.parts.retain(|part| {
                matches!(part,
                MessagePart::ToolResult { result } if keep.contains(&result.call_id.0))
            }),
        }
    }
    history.retain(|message| !message.parts.is_empty());
    history
}

fn retain_current_acceptance(history: &[Message], keep: &mut BTreeSet<String>) {
    let start = history
        .iter()
        .rposition(|message| message.role == MessageRole::User)
        .unwrap_or(0);
    let calls = history[start..]
        .iter()
        .flat_map(|message| &message.parts)
        .filter_map(|part| match part {
            MessagePart::ToolCall { request } => Some((request.call_id.0.as_str(), &request.name)),
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    if let Some(id) = history[start..].iter().rev().flat_map(|message| message.parts.iter().rev())
        .find_map(|part| match part {
            MessagePart::ToolResult { result }
                if calls.get(result.call_id.0.as_str()).is_some_and(|name| matches!(name, ipc_types::ToolName::OrchestrationSubmitPlan))
                    && matches!(&result.outcome, ToolOutcome::Completed { content } if content.iter().any(|item|
                        matches!(item, ToolResultPart::Json { value: JsonValue::Object(object) } if object.get("accepted") == Some(&JsonValue::Bool(true))))) => Some(result.call_id.0.clone()),
            _ => None,
        }) {
        keep.insert(id);
    }
}

#[cfg(test)]
mod tests;
