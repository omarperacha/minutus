//! Strict rendering of model-neutral prompts into Glimmer's ATEM format.

mod render;
mod stream;
mod validate;

use std::{error::Error, fmt};

pub(crate) use render::render_glimmer_prompt;
#[cfg(test)]
pub(crate) use stream::GlimmerStreamError;
pub(crate) use stream::GlimmerStreamParser;

use crate::{
    ModelAdapter, ModelAdapterKind, ModelOutputFailureKind, PromptAdapterError,
    PromptAdapterFailureKind, PromptRenderRequest,
};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod stream_tests;

/// Glimmer-specific prompt and output protocol implementation.
pub(crate) struct GlimmerAdapter;

impl ModelAdapter for GlimmerAdapter {
    type Decoder = GlimmerStreamParser;

    fn kind(&self) -> ModelAdapterKind {
        ModelAdapterKind::GlimmerAtem
    }

    fn render(&self, request: &PromptRenderRequest) -> Result<String, PromptAdapterError> {
        render_glimmer_prompt(request).map_err(|error| PromptAdapterError::from(&error))
    }

    fn decoder(&self, generation_id: &str) -> Result<Self::Decoder, ModelOutputFailureKind> {
        GlimmerStreamParser::new(generation_id)
            .map_err(|error| ModelOutputFailureKind::from(&error))
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ContentRole {
    System,
    User,
    Assistant,
    Tool,
}

/// Failure while validating or rendering a Glimmer prompt.
#[derive(Debug)]
pub enum GlimmerRenderError {
    /// The conversation has no messages or exceeds the history limit.
    InvalidMessageCount,
    /// The function registry exceeds the supported limit.
    TooManyTools,
    /// A message appears in an invalid role/order position.
    InvalidSequence(usize),
    /// Generation was requested after an invalid final history item.
    InvalidGenerationPoint,
    /// A message content collection or text item is empty.
    EmptyContent,
    /// A content kind is not valid for its role.
    UnsupportedContent,
    /// Assistant final content and tool calls were mixed.
    MixedAssistantOutput(usize),
    /// Text attempted to contain a renderer-owned protocol delimiter.
    ReservedDelimiter,
    /// Prompt content exceeds the bounded input limit.
    InputTooLarge,
    /// The rendered prompt exceeds its bounded output limit.
    OutputTooLarge,
    /// The supplied current date is not strict `YYYY-MM-DD`.
    InvalidCurrentDate,
    /// A recipient/function name is unsafe or malformed.
    InvalidRecipient(String),
    /// An ATEM parameter name is unsafe or malformed.
    InvalidParameter(String),
    /// A tool schema is not a JSON object.
    InvalidToolSchema(String),
    /// A tool name appears more than once.
    DuplicateTool(String),
    /// A historical call references a function outside the registry.
    UnknownTool(String),
    /// A tool-call identifier is empty, excessive or contains controls.
    InvalidToolCallId,
    /// A historical tool-call identifier appears more than once.
    DuplicateToolCall(String),
    /// A tool result does not match an outstanding historical call.
    UnknownToolCall(String),
    /// One or more tool calls have no result.
    UnresolvedToolCalls,
    /// JSON serialization failed.
    Json(serde_json::Error),
}

impl fmt::Display for GlimmerRenderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMessageCount => formatter.write_str("the prompt message count is invalid"),
            Self::TooManyTools => formatter.write_str("the prompt declares too many tools"),
            Self::InvalidSequence(index) => write!(formatter, "message {index} is out of sequence"),
            Self::InvalidGenerationPoint => {
                formatter.write_str("generation cannot start after the final history message")
            }
            Self::EmptyContent => formatter.write_str("prompt message content cannot be empty"),
            Self::UnsupportedContent => {
                formatter.write_str("the message contains content unsupported for its role")
            }
            Self::MixedAssistantOutput(index) => write!(
                formatter,
                "assistant message {index} mixes final content with tool calls"
            ),
            Self::ReservedDelimiter => {
                formatter.write_str("message content contains a reserved Glimmer delimiter")
            }
            Self::InputTooLarge => formatter.write_str("the prompt input is too large"),
            Self::OutputTooLarge => formatter.write_str("the rendered prompt is too large"),
            Self::InvalidCurrentDate => formatter.write_str("the prompt current date is invalid"),
            Self::InvalidRecipient(value) => write!(formatter, "invalid tool recipient: {value}"),
            Self::InvalidParameter(value) => write!(formatter, "invalid ATEM parameter: {value}"),
            Self::InvalidToolSchema(value) => write!(formatter, "invalid schema for tool: {value}"),
            Self::DuplicateTool(value) => write!(formatter, "duplicate tool: {value}"),
            Self::UnknownTool(value) => write!(formatter, "unknown tool: {value}"),
            Self::InvalidToolCallId => formatter.write_str("the tool-call identifier is invalid"),
            Self::DuplicateToolCall(value) => write!(formatter, "duplicate tool call: {value}"),
            Self::UnknownToolCall(value) => write!(formatter, "unknown tool call: {value}"),
            Self::UnresolvedToolCalls => formatter.write_str("one or more tool calls need results"),
            Self::Json(_) => formatter.write_str("prompt JSON could not be serialized"),
        }
    }
}

impl Error for GlimmerRenderError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<&GlimmerRenderError> for PromptAdapterError {
    fn from(error: &GlimmerRenderError) -> Self {
        use GlimmerRenderError as Glimmer;
        let kind = match error {
            Glimmer::InvalidMessageCount
            | Glimmer::InvalidSequence(_)
            | Glimmer::InvalidGenerationPoint
            | Glimmer::MixedAssistantOutput(_)
            | Glimmer::UnresolvedToolCalls => PromptAdapterFailureKind::InvalidHistory,
            Glimmer::TooManyTools => PromptAdapterFailureKind::ToolLimit,
            Glimmer::EmptyContent | Glimmer::UnsupportedContent | Glimmer::ReservedDelimiter => {
                PromptAdapterFailureKind::InvalidContent
            }
            Glimmer::InputTooLarge => PromptAdapterFailureKind::InputLimit,
            Glimmer::OutputTooLarge => PromptAdapterFailureKind::OutputLimit,
            Glimmer::InvalidCurrentDate => PromptAdapterFailureKind::InvalidDate,
            Glimmer::InvalidRecipient(_)
            | Glimmer::InvalidParameter(_)
            | Glimmer::InvalidToolSchema(_)
            | Glimmer::DuplicateTool(_) => PromptAdapterFailureKind::InvalidToolDefinition,
            Glimmer::UnknownTool(_)
            | Glimmer::InvalidToolCallId
            | Glimmer::DuplicateToolCall(_)
            | Glimmer::UnknownToolCall(_) => PromptAdapterFailureKind::InvalidToolCall,
            Glimmer::Json(_) => PromptAdapterFailureKind::Serialization,
        };
        Self::new(kind)
    }
}
