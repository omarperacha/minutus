//! Conversation and message view models.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    AssetId, ConversationId, FilesystemUndoChangeRecord, FilesystemUndoResult, MessageId,
    ModelSelection, ProjectId, TaskActivity, ToolCallId, ToolRequest, ToolResult, WorkspaceGrantId,
};

/// Lightweight conversation metadata for lists and navigation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationSummary {
    /// Stable conversation identifier.
    pub id: ConversationId,
    /// Project that owns the conversation.
    pub project_id: ProjectId,
    /// User-visible conversation title.
    pub title: String,
    /// RFC 3339 timestamp of the most recent change.
    pub updated_at: String,
}

/// A conversation and its ordered messages.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Conversation {
    /// Stable conversation identifier.
    pub id: ConversationId,
    /// Project that owns the conversation and determines its folder settings.
    pub project_id: ProjectId,
    /// User-visible conversation title.
    pub title: String,
    /// Ordered message records.
    pub messages: Vec<Message>,
    /// RFC 3339 creation timestamp.
    pub created_at: String,
    /// RFC 3339 timestamp of the most recent change.
    pub updated_at: String,
}

/// Bounded request to rename one persisted conversation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RenameConversationRequest {
    /// Conversation to update.
    pub conversation_id: ConversationId,
    /// New user-visible title.
    pub title: String,
}

/// Author role of a persisted message.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum MessageRole {
    /// Human-authored content.
    User,
    /// Model-authored content.
    Assistant,
    /// Trusted tool result content.
    Tool,
}

/// Frontend-safe content within a message.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum MessagePart {
    /// Trusted task activity displayed outside assistant prose.
    Activity {
        /// Updateable timeline record.
        activity: TaskActivity,
    },
    /// Model-authored reasoning shown separately from the final answer.
    Reasoning {
        /// UTF-8 reasoning text.
        text: String,
    },
    /// Plain text content.
    Text {
        /// UTF-8 text.
        text: String,
    },
    /// Reference to a managed local asset.
    Asset {
        /// Opaque asset identifier.
        asset_id: AssetId,
    },
    /// Tool call proposed by the model.
    ToolCall {
        /// Validated tool request.
        request: ToolRequest,
    },
    /// Result returned from a trusted tool broker.
    ToolResult {
        /// Frontend-safe result.
        result: ToolResult,
    },
    /// Persisted outcome from a user-requested filesystem undo.
    FilesystemUndo {
        /// Original direct-write tool call.
        write_call_id: ToolCallId,
        /// Per-file undo outcomes.
        result: FilesystemUndoResult,
    },
    /// Native provenance needed to bind a write to its original workspace grant.
    FilesystemWriteRecord {
        /// Persisted direct-write invocation.
        write_call_id: ToolCallId,
        /// Exact project workspace grant used for the write.
        grant_id: WorkspaceGrantId,
        /// Bounded prior states retained outside model tool results.
        changes: Vec<FilesystemUndoChangeRecord>,
    },
}

/// One persisted conversation message.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Message {
    /// Stable message identifier.
    pub id: MessageId,
    /// Parent conversation identifier.
    pub conversation_id: ConversationId,
    /// Message author role.
    pub role: MessageRole,
    /// Ordered message content.
    pub parts: Vec<MessagePart>,
    /// RFC 3339 creation timestamp.
    pub created_at: String,
}

/// Optional user choice for reasoning guidance and per-round output allowance.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum ChatEffort {
    /// Shorter per-round output allowance and low reasoning guidance.
    Quick,
    /// Moderate per-round output allowance and reasoning guidance.
    Balanced,
    /// Full per-round output allowance and high reasoning guidance.
    Deep,
}

/// Bounded request to add a user message and begin generation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChatRequest {
    /// Active project for a new chat or ownership check for an existing chat.
    pub project_id: ProjectId,
    /// Existing conversation, or absent when creating a new one.
    pub conversation_id: Option<ConversationId>,
    /// User-authored UTF-8 text.
    pub text: String,
    /// Managed local assets explicitly attached by the user.
    pub asset_ids: Vec<AssetId>,
    /// Optional installed model selected in the composer for this conversation.
    #[serde(default)]
    pub model_selection: Option<ModelSelection>,
    /// Selected total context capacity; absent uses the model's recommendation.
    #[serde(default)]
    pub context_tokens: Option<u32>,
    /// Explicit per-round reasoning and output policy; absent uses host selection.
    #[serde(default)]
    pub effort: Option<ChatEffort>,
}
