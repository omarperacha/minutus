//! Model-neutral prompt values shared by inference backends.

use std::collections::BTreeMap;

use serde_json::Value;

/// Model-neutral content accepted by a prompt adapter.
#[derive(Clone, Debug, PartialEq)]
pub enum PromptContent {
    /// UTF-8 text.
    Text(String),
    /// One already-validated managed image attachment.
    Image,
    /// Structured JSON content, normally returned by a tool.
    Json(Value),
}

/// One model-neutral historical tool invocation.
#[derive(Clone, Debug, PartialEq)]
pub struct PromptToolCall {
    /// Stable call identifier used to associate the later result.
    pub call_id: String,
    /// Recipient/function name from the active tool registry.
    pub name: String,
    /// Deterministically ordered arguments.
    pub arguments: BTreeMap<String, Value>,
}

/// One model-neutral conversation message.
#[derive(Clone, Debug, PartialEq)]
pub enum PromptMessage {
    /// Optional leading application instruction.
    System {
        /// Ordered instruction content.
        content: Vec<PromptContent>,
    },
    /// Human-authored content or a non-persisted host tail rendered in the user channel.
    User {
        /// Ordered text and managed-image markers.
        content: Vec<PromptContent>,
    },
    /// Model-authored reasoning, final content, or tool calls.
    Assistant {
        /// Optional historical reasoning addressed to the model itself.
        reasoning: Option<String>,
        /// Final content addressed to the user; empty for a tool-use message.
        content: Vec<PromptContent>,
        /// Historical tool calls; empty for a final-answer message.
        tool_calls: Vec<PromptToolCall>,
    },
    /// Untrusted result for an earlier tool call.
    ///
    /// Renderers must preserve this role boundary and must not present result
    /// content as application policy or model-authored reasoning.
    Tool {
        /// Exact identifier of the historical call receiving this result.
        call_id: String,
        /// Ordered bounded but untrusted result content.
        content: Vec<PromptContent>,
    },
}

/// One function exposed to the active model in JSON Schema form.
#[derive(Clone, Debug, PartialEq)]
pub struct PromptToolDefinition {
    /// Namespaced recipient/function name.
    pub name: String,
    /// Human-readable function description.
    pub description: String,
    /// JSON Schema object describing accepted parameters.
    pub parameters: Value,
}

/// Model-neutral reasoning directive.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReasoningStrength {
    /// Disable thinking when the backend provides a hard switch.
    Disabled,
    /// Minimize hidden reasoning.
    Low,
    /// Use moderate hidden reasoning.
    Medium,
    /// Use the backend's default high reasoning effort.
    High,
}

impl ReasoningStrength {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    /// Stable content-free diagnostic name.
    #[must_use]
    pub const fn diagnostic_name(self) -> &'static str {
        self.as_str()
    }
}

/// Complete model-neutral input to a prompt renderer.
#[derive(Clone, Debug, PartialEq)]
pub struct PromptRenderRequest {
    /// Ordered conversation history.
    pub messages: Vec<PromptMessage>,
    /// Functions available for this generation.
    pub tools: Vec<PromptToolDefinition>,
    /// Tool recipients referenced by history but unavailable for this generation.
    ///
    /// These names are validated but are never advertised to the model.
    pub historical_tool_names: Vec<String>,
    /// Current date in strict `YYYY-MM-DD` form.
    pub current_date: String,
    /// Requested reasoning strength.
    pub reasoning_strength: ReasoningStrength,
    /// Whether to append the backend's assistant generation prefix.
    pub add_generation_prompt: bool,
}
