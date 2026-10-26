//! Derivation of bounded inference history from complete persisted conversations.

use std::{collections::BTreeSet, ops::Range};

use agent_core::{UntrustedWorkingContextSummary, WorkingContextError};
#[cfg(test)]
use agent_core::{WorkingContextCompactor, WorkingContextPolicy, WorkingContextRound};
use ipc_types::{Conversation, JsonValue, Message, MessagePart, MessageRole, ToolOutcome};

pub(super) mod continuity;
pub(crate) mod recall;
pub(super) mod rich_evidence;
pub(super) mod semantic;

#[cfg(test)]
const MAX_HISTORY_TOKENS: u64 = 16_384;
#[cfg(test)]
const RECENT_TURN_TOKENS: u64 = 8_192;
#[cfg(test)]
const SUMMARY_TOKENS: u64 = 4_096;
#[cfg(test)]
const MESSAGE_OVERHEAD_TOKENS: u64 = 64;
const MAX_RECAP_ITEMS: usize = 4;
const MAX_RECAP_ITEM_BYTES: usize = 240;
const MAX_ORIGINAL_REQUEST_BYTES: usize = 700;

pub(super) struct WorkingConversation {
    message_range: Range<usize>,
    compacted_message_range: Option<Range<usize>>,
    fallback_summary: Option<UntrustedWorkingContextSummary>,
}

impl WorkingConversation {
    pub(super) fn message_range(&self) -> Range<usize> {
        self.message_range.clone()
    }

    pub(super) fn compacted_message_range(&self) -> Option<Range<usize>> {
        self.compacted_message_range.clone()
    }

    pub(super) fn fallback_summary(&self) -> Option<&UntrustedWorkingContextSummary> {
        self.fallback_summary.as_ref()
    }
}

#[cfg(test)]
pub(super) fn derive(
    conversation: &Conversation,
) -> Result<WorkingConversation, WorkingContextError> {
    let rounds = conversation_rounds(&conversation.messages)?;
    let policy = WorkingContextPolicy::new(MAX_HISTORY_TOKENS, RECENT_TURN_TOKENS, SUMMARY_TOKENS)?;
    let plan = match WorkingContextCompactor::new(policy).compact(&rounds) {
        Ok(plan) => plan,
        Err(WorkingContextError::RecentRoundTooLarge { .. }) => {
            return derive_with_oversized_latest_round(conversation, &rounds);
        }
        Err(error) => return Err(error),
    };
    if !plan.is_compacted() {
        return Ok(WorkingConversation {
            message_range: 0..conversation.messages.len(),
            compacted_message_range: None,
            fallback_summary: None,
        });
    }

    let compacted = plan.compacted_rounds();
    let recent = plan.recent_rounds();
    let compacted_messages = rounds[compacted.start].message_range().start
        ..rounds[compacted.end - 1].message_range().end;
    let recent_messages =
        rounds[recent.start].message_range().start..rounds[recent.end - 1].message_range().end;
    let summary = summarize(conversation, compacted_messages.clone())?;
    if summary.token_estimate() > plan.summary_token_budget() {
        return Err(WorkingContextError::InvalidSummary);
    }
    Ok(WorkingConversation {
        message_range: recent_messages,
        compacted_message_range: Some(compacted_messages),
        fallback_summary: Some(summary),
    })
}

#[cfg(test)]
fn derive_with_oversized_latest_round(
    conversation: &Conversation,
    rounds: &[WorkingContextRound],
) -> Result<WorkingConversation, WorkingContextError> {
    let latest = rounds.last().ok_or(WorkingContextError::InvalidRound)?;
    let recent_messages = latest.message_range();
    if rounds.len() == 1 {
        // The planning estimate is deliberately conservative and is not the
        // admission authority. Keep the active user/tool round whole and let
        // exact tokenizer-driven admission decide whether the rendered prompt
        // fits the loaded model.
        return Ok(WorkingConversation {
            message_range: recent_messages,
            compacted_message_range: None,
            fallback_summary: None,
        });
    }

    let compacted_messages = rounds[0].message_range().start..recent_messages.start;
    let summary = summarize(conversation, compacted_messages.clone())?;
    if summary.token_estimate() > SUMMARY_TOKENS {
        return Err(WorkingContextError::InvalidSummary);
    }
    Ok(WorkingConversation {
        message_range: recent_messages,
        compacted_message_range: Some(compacted_messages),
        fallback_summary: Some(summary),
    })
}

#[cfg(test)]
fn conversation_rounds(
    messages: &[Message],
) -> Result<Vec<WorkingContextRound>, WorkingContextError> {
    let starts = messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| (message.role == MessageRole::User).then_some(index))
        .collect::<Vec<_>>();
    if starts.first() != Some(&0) {
        return Err(WorkingContextError::InvalidRound);
    }
    starts
        .iter()
        .enumerate()
        .map(|(round_index, start)| {
            let end = starts
                .get(round_index + 1)
                .copied()
                .unwrap_or(messages.len());
            let token_estimate =
                messages[*start..end]
                    .iter()
                    .try_fold(0_u64, |total, message| {
                        let bytes = serde_json::to_vec(message)
                            .map_err(|_| WorkingContextError::InvalidRound)?
                            .len();
                        let bytes = u64::try_from(bytes)
                            .map_err(|_| WorkingContextError::EstimateOverflow)?;
                        total
                            .checked_add(bytes)
                            .and_then(|value| value.checked_add(MESSAGE_OVERHEAD_TOKENS))
                            .ok_or(WorkingContextError::EstimateOverflow)
                    })?;
            WorkingContextRound::new(messages[*start].id.0.clone(), *start..end, token_estimate)
        })
        .collect()
}

fn summarize(
    conversation: &Conversation,
    compacted_messages: Range<usize>,
) -> Result<UntrustedWorkingContextSummary, WorkingContextError> {
    let original_request = conversation
        .messages
        .iter()
        .find(|message| message.role == MessageRole::User)
        .and_then(first_text)
        .map(|text| bounded_text(text, MAX_ORIGINAL_REQUEST_BYTES))
        .ok_or(WorkingContextError::InvalidSummary)?;
    let mut constraints = Vec::new();
    let mut decisions = Vec::new();
    let mut unresolved = vec!["INCOMPLETE deterministic fallback: omitted details require exact conversation recall; do not infer missing constraints or outcomes.".to_owned()];
    let mut outcomes = Vec::new();
    let mut paths = Vec::new();
    let mut sources = Vec::new();
    let mut seen_paths = BTreeSet::new();
    let mut seen_sources = BTreeSet::new();

    for message in &conversation.messages[compacted_messages] {
        match message.role {
            MessageRole::User => {
                for text in message.parts.iter().filter_map(part_text) {
                    push_recap(&mut constraints, text);
                }
            }
            MessageRole::Assistant => {
                for part in &message.parts {
                    match part {
                        MessagePart::Text { text } => push_recap(&mut decisions, text),
                        MessagePart::ToolCall { request } => {
                            for value in request.arguments.values() {
                                collect_paths(value, &mut paths, &mut seen_paths);
                            }
                        }
                        MessagePart::Activity { .. }
                        | MessagePart::Reasoning { .. }
                        | MessagePart::Asset { .. }
                        | MessagePart::ToolResult { .. }
                        | MessagePart::FilesystemUndo { .. }
                        | MessagePart::FilesystemWriteRecord { .. } => {}
                    }
                }
            }
            MessageRole::Tool => {
                for part in &message.parts {
                    let MessagePart::ToolResult { result } = part else {
                        continue;
                    };
                    match &result.outcome {
                        ToolOutcome::Completed { content } => {
                            push_recap(
                                &mut outcomes,
                                &format!("Tool call {} reported completion", result.call_id.0),
                            );
                            for part in content {
                                if let ipc_types::ToolResultPart::Asset { asset_id } = part {
                                    push_unique(
                                        &mut sources,
                                        &mut seen_sources,
                                        asset_id.0.as_str(),
                                    );
                                }
                            }
                        }
                        ToolOutcome::Failed { error } => {
                            push_recap(
                                &mut unresolved,
                                &format!(
                                    "Tool call {} failed: {}",
                                    result.call_id.0, error.message
                                ),
                            );
                        }
                    }
                }
            }
        }
        for part in &message.parts {
            if let MessagePart::Asset { asset_id } = part {
                push_unique(&mut sources, &mut seen_sources, asset_id.0.as_str());
            }
        }
    }

    UntrustedWorkingContextSummary::new(
        original_request,
        constraints,
        decisions,
        unresolved,
        outcomes,
        paths,
        sources,
    )
}

fn first_text(message: &Message) -> Option<&str> {
    message.parts.iter().find_map(part_text)
}

fn part_text(part: &MessagePart) -> Option<&str> {
    match part {
        MessagePart::Text { text } => Some(text),
        MessagePart::Activity { .. }
        | MessagePart::Reasoning { .. }
        | MessagePart::Asset { .. }
        | MessagePart::ToolCall { .. }
        | MessagePart::ToolResult { .. }
        | MessagePart::FilesystemUndo { .. }
        | MessagePart::FilesystemWriteRecord { .. } => None,
    }
}

fn collect_paths(value: &JsonValue, paths: &mut Vec<String>, seen: &mut BTreeSet<String>) {
    match value {
        JsonValue::Object(values) => {
            for (key, value) in values {
                if matches!(key.as_str(), "path" | "relativePath")
                    && let JsonValue::String(path) = value
                {
                    push_unique(paths, seen, path);
                }
                collect_paths(value, paths, seen);
            }
        }
        JsonValue::Array(values) => {
            for value in values {
                collect_paths(value, paths, seen);
            }
        }
        JsonValue::Null | JsonValue::Bool(_) | JsonValue::Number(_) | JsonValue::String(_) => {}
    }
}

fn push_recap(values: &mut Vec<String>, value: &str) {
    if values.len() < MAX_RECAP_ITEMS {
        let value = bounded_text(value, MAX_RECAP_ITEM_BYTES);
        if !value.is_empty() && !values.contains(&value) {
            values.push(value);
        }
    }
}

fn push_unique(values: &mut Vec<String>, seen: &mut BTreeSet<String>, value: &str) {
    if values.len() >= MAX_RECAP_ITEMS {
        return;
    }
    let value = bounded_text(value, MAX_RECAP_ITEM_BYTES);
    if !value.is_empty() && seen.insert(value.clone()) {
        values.push(value);
    }
}

fn bounded_text(value: &str, max_bytes: usize) -> String {
    let sanitized = value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let sanitized = sanitized.trim();
    if sanitized.len() <= max_bytes {
        return sanitized.to_owned();
    }
    let mut end = max_bytes;
    while !sanitized.is_char_boundary(end) {
        end -= 1;
    }
    sanitized[..end].trim_end().to_owned()
}

#[cfg(test)]
mod tests;
