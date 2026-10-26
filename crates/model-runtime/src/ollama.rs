//! Bounded loopback-only Ollama transport and pinned-model registration.

use std::{
    collections::{BTreeMap, HashMap},
    error::Error,
    fmt, fs,
    io::{BufRead, BufReader},
    net::SocketAddr,
    path::Path,
    sync::Mutex,
    time::{Duration, Instant},
};

use reqwest::{StatusCode, blocking::Client};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    DownloadCancellation, DownloadPhase, DownloadProgress, GenerationCompletion, GenerationEvent,
    ModelManifest, ModelOutputDecoder, ModelOutputFailureKind, PromptToolDefinition,
    ReasoningStrength, blocking_client::BlockingClient,
};

const MAX_CONTROL_ATTEMPTS: usize = 3;
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_STREAM_LINE_BYTES: usize = 1024 * 1024;
const CHAT_DEADLINE: Duration = Duration::from_mins(30);
const PULL_DEADLINE: Duration = Duration::from_hours(6);

mod config;
mod sampling;
mod stream;
mod tuning;
pub(crate) use tuning::OLLAMA_PARALLEL_CONTEXTS;
pub use tuning::{QwenRuntimeProfile, QwenTuningPolicy};

pub use config::{OllamaProviderConfig, VerifiedOllamaModel};

/// Minimal structured message accepted by the bounded chat transport.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OllamaChatMessage {
    role: String,
    content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    images: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<OllamaToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

impl OllamaChatMessage {
    /// Creates a message with one of Ollama's closed text roles.
    ///
    /// # Errors
    ///
    /// Rejects roles outside the closed Ollama chat-role set.
    pub fn new(role: &str, content: impl Into<String>) -> Result<Self, OllamaError> {
        if !matches!(role, "system" | "user" | "assistant" | "tool") {
            return Err(OllamaError::InvalidRequest);
        }
        Ok(Self {
            role: role.to_owned(),
            content: content.into(),
            thinking: None,
            images: Vec::new(),
            tool_calls: Vec::new(),
            tool_name: None,
            tool_call_id: None,
        })
    }

    pub(crate) fn native(
        role: &'static str,
        content: String,
        thinking: Option<String>,
        images: Vec<String>,
        tool_calls: Vec<OllamaToolCall>,
        tool_name: Option<String>,
        tool_call_id: Option<String>,
    ) -> Self {
        Self {
            role: role.to_owned(),
            content,
            thinking,
            images,
            tool_calls,
            tool_name,
            tool_call_id,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct OllamaToolCall {
    id: String,
    function: OllamaToolFunctionCall,
}

impl OllamaToolCall {
    pub(crate) fn new(id: String, name: String, arguments: Value) -> Self {
        Self {
            id,
            function: OllamaToolFunctionCall { name, arguments },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct OllamaToolFunctionCall {
    name: String,
    arguments: Value,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct OllamaToolDefinition {
    r#type: &'static str,
    function: OllamaToolFunctionDefinition,
}

impl OllamaToolDefinition {
    pub(crate) fn new(name: String, description: String, parameters: Value) -> Self {
        Self {
            r#type: "function",
            function: OllamaToolFunctionDefinition {
                name,
                description,
                parameters,
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct OllamaToolFunctionDefinition {
    name: String,
    description: String,
    parameters: Value,
}

/// Typed chat transport request. QWEN-004 owns the richer native rendering.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OllamaChatRequest {
    model: String,
    messages: Vec<OllamaChatMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OllamaToolDefinition>,
    stream: bool,
    think: OllamaThink,
    #[serde(skip)]
    sampling: Option<sampling::QwenSampling>,
    #[serde(skip_serializing_if = "Option::is_none")]
    truncate: Option<bool>,
    shift: WireBool,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<OllamaChatOptions>,
    #[serde(rename = "_debug_render_only", skip_serializing_if = "is_false")]
    debug_render_only: bool,
}

// The pinned Ollama chat path explicitly enables `shift`, which Ollama 0.32.14
// maps to llama.cpp `cache_prompt: true`. One resident runner and one parallel
// context then retain the longest reusable prefix. Context overflow remains
// rejected by Zeta's independent `truncate: false` admission setting.

/// Raw prompt request used by adapters that own their complete wire format.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OllamaGenerateRequest {
    model: String,
    prompt: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    images: Vec<String>,
    stream: WireBool,
    raw: WireBool,
    think: WireBool,
    truncate: WireBool,
    shift: WireBool,
    options: OllamaChatOptions,
}

impl OllamaGenerateRequest {
    /// Creates a raw, non-truncating request for an already-rendered adapter prompt.
    #[must_use]
    pub fn raw(
        model: &VerifiedOllamaModel,
        prompt: impl Into<String>,
        images: Vec<String>,
        context_tokens: u32,
        output_tokens: u32,
    ) -> Self {
        Self {
            model: model.tag.clone(),
            prompt: prompt.into(),
            images,
            stream: WireBool(true),
            raw: WireBool(true),
            think: WireBool(false),
            truncate: WireBool(false),
            shift: WireBool(false),
            options: OllamaChatOptions {
                num_ctx: context_tokens,
                num_predict: output_tokens,
                draft_num_predict: None,
                sampling: None,
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct OllamaChatOptions {
    num_ctx: u32,
    num_predict: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    draft_num_predict: Option<u32>,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    sampling: Option<sampling::QwenSampling>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
struct WireBool(bool);

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(untagged)]
enum OllamaThink {
    Enabled(bool),
    Strength(String),
}

impl OllamaChatRequest {
    /// Creates a streaming request with explicit reasoning behavior.
    #[must_use]
    pub fn new(model: &VerifiedOllamaModel, messages: Vec<OllamaChatMessage>, think: bool) -> Self {
        Self {
            model: model.tag.clone(),
            messages,
            tools: Vec::new(),
            stream: true,
            think: OllamaThink::Enabled(think),
            sampling: None,
            truncate: None,
            shift: WireBool(true),
            options: None,
            debug_render_only: false,
        }
    }

    pub(crate) fn native(
        model: &VerifiedOllamaModel,
        messages: Vec<OllamaChatMessage>,
        tools: Vec<OllamaToolDefinition>,
        reasoning: ReasoningStrength,
    ) -> Self {
        let think = match reasoning {
            ReasoningStrength::Disabled => OllamaThink::Enabled(false),
            ReasoningStrength::Low => OllamaThink::Strength("low".to_owned()),
            ReasoningStrength::Medium => OllamaThink::Strength("medium".to_owned()),
            ReasoningStrength::High => OllamaThink::Strength("high".to_owned()),
        };
        Self {
            model: model.tag.clone(),
            messages,
            tools,
            stream: true,
            think,
            sampling: Some(sampling::QwenSampling::for_thinking(
                reasoning != ReasoningStrength::Disabled,
            )),
            truncate: None,
            shift: WireBool(true),
            options: None,
            debug_render_only: false,
        }
    }

    /// Carries a fully rendered adapter prompt through an identity chat template
    /// while still supplying structured tool definitions to Ollama's parser.
    #[must_use]
    pub fn adapter_prompt(
        model: &VerifiedOllamaModel,
        prompt: String,
        images: Vec<String>,
        tools: &[PromptToolDefinition],
    ) -> Self {
        Self {
            model: model.tag.clone(),
            messages: vec![OllamaChatMessage::native(
                "user",
                prompt,
                None,
                images,
                Vec::new(),
                None,
                None,
            )],
            tools: tools
                .iter()
                .map(|tool| {
                    OllamaToolDefinition::new(
                        tool.name.clone(),
                        tool.description.clone(),
                        tool.parameters.clone(),
                    )
                })
                .collect(),
            stream: true,
            think: OllamaThink::Enabled(true),
            sampling: None,
            truncate: None,
            shift: WireBool(true),
            options: None,
            debug_render_only: false,
        }
    }

    /// Pins the exact host-admitted window and forbids Ollama history truncation.
    #[must_use]
    pub fn with_context(mut self, context_tokens: u32, output_tokens: u32) -> Self {
        self.truncate = Some(false);
        self.options = Some(OllamaChatOptions {
            num_ctx: context_tokens,
            num_predict: output_tokens,
            draft_num_predict: None,
            sampling: self.sampling,
        });
        self
    }
}

/// One bounded decoded transport chunk; model-specific parsing remains adapter-owned.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct OllamaStreamChunk {
    /// Whether Ollama marked this as the final stream object.
    #[serde(default)]
    pub done: bool,
    /// Structured message fragment, when present.
    #[serde(default)]
    pub message: Option<OllamaStreamMessage>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct OllamaStreamMessage {
    /// User-visible content fragment.
    #[serde(default)]
    pub content: String,
    /// Private reasoning fragment kept separate from final content.
    #[serde(default)]
    pub thinking: String,
}

#[derive(Debug, Deserialize)]
struct OllamaGenerateChunk {
    #[serde(default)]
    response: String,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    done_reason: String,
}

/// Loopback-only provider with a closed set of Ollama operations.
pub struct OllamaProvider {
    config: OllamaProviderConfig,
    control_client: BlockingClient,
    pull_client: BlockingClient,
    operation: Mutex<()>,
}

mod client;

fn emit_decoder_events(
    events: Vec<GenerationEvent>,
    on_event: &mut impl FnMut(GenerationEvent),
) -> bool {
    let mut terminal = false;
    for event in events {
        terminal |= matches!(
            event,
            GenerationEvent::Completed { .. }
                | GenerationEvent::Cancelled
                | GenerationEvent::Failed { .. }
        );
        on_event(event);
    }
    !terminal
}

#[derive(Clone, Copy)]
enum Operation {
    Version,
    Tags,
    Pull,
    Chat,
    Generate,
    Create,
}

impl Operation {
    const fn path(self) -> &'static str {
        match self {
            Self::Version => "/api/version",
            Self::Tags => "/api/tags",
            Self::Pull => "/api/pull",
            Self::Chat => "/api/chat",
            Self::Generate => "/api/generate",
            Self::Create => "/api/create",
        }
    }
}

#[derive(Deserialize)]
struct VersionResponse {
    version: String,
}

#[derive(Serialize)]
struct CreateRequest<'a> {
    model: &'a str,
    stream: bool,
    files: BTreeMap<String, String>,
    template: &'static str,
    parser: &'static str,
}

#[derive(Deserialize)]
struct CreateResponse {
    status: String,
}

#[derive(Deserialize)]
struct DebugChatResponse {
    #[serde(rename = "_debug_info")]
    debug_info: DebugInfo,
}

#[derive(Deserialize)]
struct DebugInfo {
    rendered_template: String,
}

#[derive(Deserialize)]
struct TagsResponse {
    #[serde(default)]
    models: Vec<TagModel>,
}

#[derive(Deserialize)]
struct TagModel {
    name: String,
    digest: String,
}

#[derive(Serialize)]
struct PullRequest<'a> {
    model: &'a str,
    stream: bool,
    insecure: bool,
}

#[derive(Deserialize)]
struct PullResponse {
    digest: Option<String>,
    completed: Option<u64>,
}

fn complete_bundle_bytes(model: &ModelManifest) -> Result<u64, OllamaError> {
    if let Some(bytes) = model
        .ollama_distribution()
        .and_then(crate::OllamaDistribution::tensor_bundle_bytes)
    {
        return Ok(bytes);
    }
    model
        .companions()
        .iter()
        .try_fold(model.artifact().installed_bytes(), |total, companion| {
            total
                .checked_add(companion.artifact().installed_bytes())
                .ok_or(OllamaError::InvalidConfiguration)
        })
}

fn normalize_digest(value: &str) -> &str {
    value.strip_prefix("sha256:").unwrap_or(value)
}

fn version_at_least(actual: &str, required: &str) -> bool {
    fn parse(value: &str) -> Option<[u64; 3]> {
        let core = value
            .trim()
            .trim_start_matches('v')
            .split(['-', '+'])
            .next()?;
        let mut parts = core.split('.');
        let parsed = [
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
        ];
        (parts.next().is_none()).then_some(parsed)
    }
    matches!((parse(actual), parse(required)), (Some(actual), Some(required)) if actual >= required)
}

fn classify_status(status: StatusCode) -> OllamaError {
    if status == StatusCode::NOT_FOUND {
        OllamaError::ModelUnavailable
    } else {
        OllamaError::ServerRejected
    }
}

/// Sanitized provider failures. Response bodies and generated text are never retained.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OllamaError {
    /// The endpoint or pinned profile is outside the supported configuration.
    InvalidConfiguration,
    /// A typed request contains an unsupported value.
    InvalidRequest,
    /// The loopback server could not be reached.
    Unavailable,
    /// The server version predates the pinned compatibility floor.
    IncompatibleServer,
    /// The exact pinned tag and digest are not registered.
    ModelUnavailable,
    /// The server rejected a bounded operation.
    ServerRejected,
    /// A bounded response was malformed or too large.
    InvalidResponse,
    /// A streaming operation exceeded its total deadline.
    DeadlineExceeded,
    /// The caller cooperatively cancelled the operation.
    Cancelled,
}

impl fmt::Display for OllamaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidConfiguration => "the Ollama provider configuration is invalid",
            Self::InvalidRequest => "the Ollama request is invalid",
            Self::Unavailable => "the local Ollama server is unavailable",
            Self::IncompatibleServer => "the local Ollama server version is incompatible",
            Self::ModelUnavailable => "the pinned Ollama model is unavailable",
            Self::ServerRejected => "the local Ollama server rejected the request",
            Self::InvalidResponse => "the local Ollama server returned an invalid response",
            Self::DeadlineExceeded => "the Ollama operation exceeded its deadline",
            Self::Cancelled => "the Ollama operation was cancelled",
        })
    }
}

impl Error for OllamaError {}

impl From<OllamaError> for ModelOutputFailureKind {
    fn from(error: OllamaError) -> Self {
        match error {
            OllamaError::DeadlineExceeded => Self::ProviderTimeout,
            OllamaError::Unavailable => Self::ProviderUnavailable,
            OllamaError::InvalidResponse => Self::InvalidProviderResponse,
            OllamaError::Cancelled => Self::Cancelled,
            OllamaError::InvalidConfiguration
            | OllamaError::InvalidRequest
            | OllamaError::IncompatibleServer
            | OllamaError::ModelUnavailable
            | OllamaError::ServerRejected => Self::ProviderRejected,
        }
    }
}

#[cfg(test)]
mod tests;
