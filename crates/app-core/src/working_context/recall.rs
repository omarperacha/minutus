//! Bounded exact historical recall. Never executes stored requests or grants authority.

use agent_core::{CompactRehydrationContext, HostSourceBinding, WorkingContextError};
use ipc_types::{Conversation, Message, MessagePart, ToolOutcome, ToolResultPart};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{AppCore, AppCoreError};

const PAGE_SIZE: usize = 16;
const MAX_SLICE_BYTES: usize = 8192;

struct Entry<'a> {
    message: &'a Message,
    part: &'a MessagePart,
    reference: String,
}

fn entries(conversation: &Conversation) -> Result<Vec<Entry<'_>>, AppCoreError> {
    let mut entries = Vec::new();
    for message in &conversation.messages {
        if message.conversation_id != conversation.id {
            return Err(WorkingContextError::InvalidCheckpoint.into());
        }
        for (index, part) in message.parts.iter().enumerate() {
            if !matches!(
                part,
                MessagePart::Text { .. }
                    | MessagePart::ToolResult { .. }
                    | MessagePart::Asset { .. }
            ) {
                continue;
            }
            // Scope, message identity, part position and content all participate.
            // Neither a changed input nor another conversation can reuse this ID.
            let encoded = serde_json::to_vec(&(
                &conversation.project_id,
                &conversation.id,
                &message.id,
                index,
                part,
            ))
            .map_err(|_| WorkingContextError::InvalidCheckpoint)?;
            entries.push(Entry {
                message,
                part,
                reference: format!("history:{:x}", Sha256::digest(encoded)),
            });
        }
    }
    Ok(entries)
}

/// One paginated selection manifest shared by prompt rendering and recall.
/// `None` means the storage reader has no knowledge of the active prompt.
pub(crate) fn manifest(
    conversation: &Conversation,
    selected: Option<&[Message]>,
    offset: usize,
) -> Result<Value, AppCoreError> {
    let entries = entries(conversation)?;
    let included = |entry: &Entry<'_>| {
        selected.is_some_and(|messages| {
            messages.iter().any(|message| {
                message.id == entry.message.id
                    && message.parts.iter().any(|part| same_part(part, entry.part))
            })
        })
    };
    let verbatim = |entry: &Entry<'_>| {
        selected.map(|messages| {
            messages
                .iter()
                .any(|message| message.id == entry.message.id && message.parts.contains(entry.part))
        })
    };
    let included_count = selected.map(|_| entries.iter().filter(|entry| included(entry)).count());
    let verbatim_count = selected.map(|_| {
        entries
            .iter()
            .filter(|entry| verbatim(entry) == Some(true))
            .count()
    });
    let operations = conversation
        .messages
        .iter()
        .flat_map(|message| &message.parts)
        .filter_map(|part| match part {
            MessagePart::ToolCall { request } => Some((&request.call_id, &request.name)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let page = entries.iter().skip(offset).take(PAGE_SIZE).map(|entry| {
        let call_id = match entry.part { MessagePart::ToolResult { result } => Some(&result.call_id), _ => None };
        json!({
            "reference": entry.reference, "messageId": entry.message.id,
            "role": entry.message.role, "createdAt": entry.message.created_at, "toolCallId": call_id,
            "toolName": call_id.and_then(|id| operations.iter().find(|(call, _)| *call == id).map(|(_, name)| *name)),
            "selection": if selected.is_none() { "unknown" } else if included(entry) { "included" } else { "retained_omitted" },
            "coverage": "one_persisted_message_part", "sourceFreshness": "historical_unverified",
            "selectedVerbatim": verbatim(entry),
            "payloadAvailability": "check_on_recall"
        })
    }).collect::<Vec<_>>();
    let next = offset.saturating_add(page.len());
    Ok(json!({
        "version": 2, "entries": page, "total": entries.len(), "offset": offset,
        "includedCount": included_count, "retainedOmittedCount": included_count.map(|count| entries.len() - count),
        "selectedVerbatimCount": verbatim_count,
        "nextOffset": (next < entries.len()).then_some(next),
        "historyOnly": true, "permissionGranted": false,
        "guidance": "Use content already visible in the conversation directly. Recall only a specific needed input that is missing or truncated; listing this index is not a prerequisite for planning or task tools. Selection is unknown in standalone index results because storage does not know the active prompt. Use orchestration.recall alone with reference empty and nextOffset to page this index; use an exact reference with offset 0 to recover bounded historical text. Retention is not proof of payload availability or current source truth. Missing inputs must be recalled or explicitly reported as gaps, never inferred."
    }))
}

impl AppCore {
    /// Reads a bounded slice or index within the caller's current conversation.
    /// Empty reference selects the paginated index. Stored calls are never run.
    ///
    /// # Errors
    /// Returns an error for invalid bounds, malformed persistence or scope mismatch.
    pub fn recall_conversation_context(
        &self,
        conversation: &Conversation,
        reference: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Value, AppCoreError> {
        if reference.len() > 160 || limit == 0 || limit > MAX_SLICE_BYTES {
            return Err(WorkingContextError::InvalidPolicy.into());
        }
        let Some(stored) = self
            .with_storage(|storage| storage.conversation(&conversation.id))?
            .filter(|stored| stored.project_id == conversation.project_id)
        else {
            return Ok(gap(
                RecallGap::Unavailable,
                "The conversation was deleted or its scope changed; this reference cannot be recalled.",
            ));
        };
        let conversation = &stored;
        if reference.is_empty() {
            return manifest(conversation, None, offset);
        }
        if !reference.starts_with("history:") {
            return self.recall_compact_payload(conversation, reference, offset, limit);
        }
        let entries = entries(conversation)?;
        let Some(entry) = entries.iter().find(|entry| entry.reference == reference) else {
            return Ok(gap(
                RecallGap::Unavailable,
                "Reference is missing, changed or outside this conversation. Page the current index to locate the exact input.",
            ));
        };
        let content = match entry.part {
            MessagePart::Text { text } => text.clone(),
            MessagePart::ToolResult { result } => {
                if let ToolOutcome::Completed { content } = &result.outcome {
                    for item in content {
                        if let ToolResultPart::Asset { asset_id } = item
                            && self.inference_asset(asset_id).is_err()
                        {
                            return Ok(gap(
                                RecallGap::Unavailable,
                                "The original managed image is unavailable. Reload it with its owning image tool or ask for the exact missing attachment.",
                            ));
                        }
                    }
                }
                serde_json::to_string(&result.outcome)
                    .map_err(|_| WorkingContextError::InvalidCheckpoint)?
            }
            MessagePart::Asset { asset_id } => {
                return Ok(if self.inference_asset(asset_id).is_ok() {
                    json!({"status": "retained", "assetId": asset_id, "pixelsIncluded": false,
                        "resolution": "Use the original attachment or image reader for pixel evidence; this reference does not establish extraction accuracy."})
                } else {
                    gap(
                        RecallGap::Unavailable,
                        "The original managed image is unavailable; reload the exact attachment.",
                    )
                });
            }
            _ => return Err(WorkingContextError::InvalidCheckpoint.into()),
        };
        Ok(slice(&content, offset, limit))
    }

    fn recall_compact_payload(
        &self,
        conversation: &Conversation,
        reference: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Value, AppCoreError> {
        let Some(record) = self.with_storage(|storage| {
            storage.compact_tool_result(reference, &conversation.id.0, &conversation.project_id.0)
        })?
        else {
            return Ok(gap(
                RecallGap::Unavailable,
                "Retained payload is absent, expired, or outside this conversation; use the original source reader.",
            ));
        };
        let now = OffsetDateTime::now_utc()
            .replace_nanosecond(0)
            .map_err(|_| WorkingContextError::InvalidCheckpoint)?
            .format(&Rfc3339)
            .map_err(|_| WorkingContextError::InvalidCheckpoint)?;
        if now.as_str() >= record.expires_at() {
            return Ok(gap(
                RecallGap::Unavailable,
                "Retained payload has expired; use the original source reader.",
            ));
        }
        // This owner cannot mint a live workspace revision or fresh network
        // receipt. Such records must go through their existing source broker.
        if record.source_bindings().iter().any(|source| {
            matches!(
                source,
                HostSourceBinding::Workspace { .. }
                    | HostSourceBinding::ExternalResponse {
                        immutable: false,
                        ..
                    }
            )
        }) {
            return Ok(gap(
                RecallGap::Stale,
                "Live source validation is required. Use the existing filesystem or web reader with the source identified by the retained envelope; do not repeat a write or payment.",
            ));
        }
        for asset in record.asset_ids() {
            if self
                .inference_asset(&ipc_types::AssetId(asset.to_owned()))
                .is_err()
            {
                return Ok(gap(
                    RecallGap::Unavailable,
                    "Retained asset bytes are missing; reload the original image source.",
                ));
            }
        }
        let context = CompactRehydrationContext::new(
            &conversation.project_id.0,
            &conversation.id.0,
            record.source_bindings().to_vec(),
        )
        .map_err(|_| WorkingContextError::InvalidCheckpoint)?;
        let Ok(payload) = record.rehydrate(&context, &now) else {
            return Ok(gap(
                RecallGap::Unavailable,
                "Retained payload expired or failed source validation; use the original source reader.",
            ));
        };
        let encoded =
            serde_json::to_string(payload).map_err(|_| WorkingContextError::InvalidCheckpoint)?;
        Ok(slice(&encoded, offset, limit))
    }
}

#[derive(Clone, Copy)]
enum RecallGap {
    Unavailable,
    Stale,
    InvalidSlice,
}

fn gap(status: RecallGap, resolution: &str) -> Value {
    let status = match status {
        RecallGap::Unavailable => "unavailable",
        RecallGap::Stale => "stale",
        RecallGap::InvalidSlice => "invalid_slice",
    };
    json!({"status": status, "contextGap": true, "resolution": resolution, "permissionGranted": false})
}

fn slice(content: &str, offset: usize, limit: usize) -> Value {
    if offset > content.len() || !content.is_char_boundary(offset) {
        return gap(
            RecallGap::InvalidSlice,
            "Use offset 0 or the exact nextOffset returned by the prior slice.",
        );
    }
    let mut end = offset.saturating_add(limit).min(content.len());
    while end > offset && !content.is_char_boundary(end) {
        end -= 1;
    }
    if end == offset && offset < content.len() {
        return gap(
            RecallGap::InvalidSlice,
            "Increase limit to at least 4 bytes to fit one Unicode character.",
        );
    }
    json!({"status": "recalled", "historical": true, "sourceFreshness": "unverified",
        "permissionGranted": false, "progressCredited": false, "offset": offset, "totalBytes": content.len(),
        "nextOffset": (end < content.len()).then_some(end), "truncated": end < content.len(),
        "content": &content[offset..end]})
}

#[cfg(test)]
mod tests;

fn same_part(selected: &MessagePart, original: &MessagePart) -> bool {
    match (selected, original) {
        (
            MessagePart::ToolResult { result: selected },
            MessagePart::ToolResult { result: original },
        ) => selected.call_id == original.call_id,
        _ => selected == original,
    }
}
