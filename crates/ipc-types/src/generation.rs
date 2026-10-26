//! Model-neutral streaming generation events.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    Conversation, ConversationId, IpcError, MessageId, RequestId, TaskActivity, ToolCallId,
    ToolRequest,
};

/// Receipt returned after a chat generation has been persisted and scheduled.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChatGenerationAccepted {
    /// Opaque generation request identifier.
    pub request_id: RequestId,
    /// Conversation including the newly persisted user message.
    pub conversation: Conversation,
}

/// Conversation-scoped event emitted during one chat generation.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChatGenerationEvent {
    /// Conversation receiving the generation.
    pub conversation_id: ConversationId,
    /// Model-neutral streamed event.
    pub event: GenerationEvent,
}

/// Reason a generation produced a final response.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum FinishReason {
    /// The model emitted its normal final condition.
    Stop,
    /// The configured generation limit was reached.
    Length,
}

/// Trusted content-free lifecycle phase for one active chat response.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ChatGenerationPhase {
    /// The request was persisted and its worker is starting.
    StartingResponse,
    /// The selected local model is being verified or made resident.
    LoadingLocalModel,
    /// Conversation history, tool contracts and exact token admission are prepared.
    PreparingConversationContext,
    /// The provider request is active before first response bytes; this may
    /// include prompt evaluation and silent generation.
    ProcessingContext,
    /// The first provider response bytes have arrived; an output channel is
    /// identified only when its corresponding delta is observed.
    Thinking,
    /// Validated model-requested operations are running.
    ExecutingTools,
    /// The final response is being committed to conversation history.
    SavingResponse,
}

/// Normalized event emitted by any supported local model adapter.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum GenerationEvent {
    /// Trusted lifecycle progress that contains no prompt or model output.
    Status {
        /// Generation request identifier.
        request_id: RequestId,
        /// Monotonic event sequence number.
        sequence: u32,
        /// Current host-observed response phase.
        phase: ChatGenerationPhase,
    },
    /// Provisional display text; never a committed answer or verified outcome.
    TextPreviewDelta {
        /// Generation request identifier.
        request_id: RequestId,
        /// Monotonic event sequence number.
        sequence: u32,
        /// Zero-based inference round, scoped to this request.
        round: u32,
        /// Bounded UTF-8 display fragment.
        text: String,
    },
    /// Clear provisional display at a new round or reclassification boundary.
    TextPreviewReset {
        /// Generation request identifier.
        request_id: RequestId,
        /// Monotonic event sequence number.
        sequence: u32,
        /// Zero-based inference round, scoped to this request.
        round: u32,
    },
    /// Trusted timeline activity update from the native agent runtime.
    Activity {
        /// Generation request identifier.
        request_id: RequestId,
        /// Monotonic event sequence number.
        sequence: u32,
        /// Activity record to insert or update.
        activity: TaskActivity,
    },
    /// Visible assistant text delta.
    TextDelta {
        /// Generation request identifier.
        request_id: RequestId,
        /// Monotonic event sequence number.
        sequence: u32,
        /// Bounded UTF-8 text.
        text: String,
    },
    /// Optional reasoning delta exposed by the active model profile.
    ReasoningDelta {
        /// Generation request identifier.
        request_id: RequestId,
        /// Monotonic event sequence number.
        sequence: u32,
        /// Bounded UTF-8 reasoning text.
        text: String,
    },
    /// Validated request for an allow-listed tool.
    ToolCall {
        /// Generation request identifier.
        request_id: RequestId,
        /// Monotonic event sequence number.
        sequence: u32,
        /// Parsed tool request.
        request: ToolRequest,
    },
    /// Signal that generation is paused until a tool result is supplied.
    ToolResultNeeded {
        /// Generation request identifier.
        request_id: RequestId,
        /// Monotonic event sequence number.
        sequence: u32,
        /// Tool invocation awaiting a result.
        call_id: ToolCallId,
    },
    /// Final assistant response.
    Final {
        /// Generation request identifier.
        request_id: RequestId,
        /// Monotonic event sequence number.
        sequence: u32,
        /// Persisted assistant message identifier.
        message_id: MessageId,
        /// Complete final response text.
        text: String,
        /// Why generation finalized.
        finish_reason: FinishReason,
    },
    /// Sanitized terminal or recoverable error.
    Error {
        /// Generation request identifier, when one was established.
        request_id: Option<RequestId>,
        /// Monotonic event sequence number.
        sequence: u32,
        /// Frontend-safe error.
        error: IpcError,
    },
    /// Generation was cancelled without further model effects.
    Cancelled {
        /// Generation request identifier.
        request_id: RequestId,
        /// Monotonic event sequence number.
        sequence: u32,
    },
}
