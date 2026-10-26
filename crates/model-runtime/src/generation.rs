//! Model-neutral generation contracts shared by adapters and providers.

use std::{collections::BTreeMap, error::Error, fmt};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::glimmer::{GlimmerAdapter, GlimmerStreamParser};
use crate::{ModelCapabilities, ModelOutputFailureKind, PromptRenderRequest};

/// Closed prompt/output adapter implementations supported by this build.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelAdapterKind {
    /// Muse Glimmer's recipient channels and ATEM tool protocol.
    GlimmerAtem,
    /// Qwen's native Ollama messages, tools, images and separated thinking.
    QwenNative,
}

impl ModelAdapterKind {
    /// Stable content-free diagnostic name.
    #[must_use]
    pub const fn diagnostic_name(self) -> &'static str {
        match self {
            Self::GlimmerAtem => "glimmer_atem",
            Self::QwenNative => "qwen_native",
        }
    }
}

/// Closed tokenizer implementations used for exact host admission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelTokenizerKind {
    /// Tokenization performed by the authenticated pinned `llama.cpp` helper.
    LlamaCpp,
    /// Exact vocabulary-only tokenization from the selected model's GGUF metadata.
    VocabularyOnly,
    /// Pinned Qwen 3.5/3.8 GPT-2 BPE admission policy.
    Qwen35Bpe,
}

/// Closed local inference providers supported by this build.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InferenceProviderKind {
    /// Application-managed, authenticated `llama.cpp` sidecar.
    LlamaCpp,
    /// Application-managed, pinned Ollama sidecar.
    Ollama,
}

impl InferenceProviderKind {
    /// Stable content-free diagnostic name.
    #[must_use]
    pub const fn diagnostic_name(self) -> &'static str {
        match self {
            Self::LlamaCpp => "llama_cpp",
            Self::Ollama => "ollama",
        }
    }
}

/// Immutable identities and capabilities bound to one loaded model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GenerationBinding {
    model_id: String,
    model_version: String,
    model_sha256: String,
    adapter: ModelAdapterKind,
    adapter_version: String,
    tokenizer: ModelTokenizerKind,
    tokenizer_version: String,
    provider: InferenceProviderKind,
    provider_version: String,
    capabilities: ModelCapabilities,
    runtime_profile: Option<crate::QwenRuntimeProfile>,
}

impl GenerationBinding {
    /// Creates the pinned Glimmer/`llama.cpp` binding used by the current provider.
    #[must_use]
    pub fn glimmer_llama_cpp(
        model_id: impl Into<String>,
        model_version: impl Into<String>,
        model_sha256: impl Into<String>,
        backend_version: impl Into<String>,
        capabilities: ModelCapabilities,
    ) -> Self {
        let backend_version = backend_version.into();
        Self {
            model_id: model_id.into(),
            model_version: model_version.into(),
            model_sha256: model_sha256.into(),
            adapter: ModelAdapterKind::GlimmerAtem,
            adapter_version: "glimmer-atem-v1".to_owned(),
            tokenizer: ModelTokenizerKind::LlamaCpp,
            tokenizer_version: backend_version.clone(),
            provider: InferenceProviderKind::LlamaCpp,
            provider_version: backend_version,
            capabilities,
            runtime_profile: None,
        }
    }

    /// Creates the Glimmer adapter over raw Ollama generation with exact shared tokenization.
    #[must_use]
    pub fn glimmer_ollama(
        model_id: impl Into<String>,
        model_version: impl Into<String>,
        model_sha256: impl Into<String>,
        provider_version: impl Into<String>,
        capabilities: ModelCapabilities,
    ) -> Self {
        Self {
            model_id: model_id.into(),
            model_version: model_version.into(),
            model_sha256: model_sha256.into(),
            adapter: ModelAdapterKind::GlimmerAtem,
            adapter_version: "glimmer-atem-v1".to_owned(),
            tokenizer: ModelTokenizerKind::VocabularyOnly,
            tokenizer_version: format!("llama.cpp-vocab-only:{}", crate::PINNED_LLAMA_CPP_VERSION),
            provider: InferenceProviderKind::Ollama,
            provider_version: provider_version.into(),
            capabilities,
            runtime_profile: None,
        }
    }

    /// Creates the pinned Qwen/native-Ollama binding.
    #[must_use]
    pub fn qwen_ollama(
        model_id: impl Into<String>,
        model_version: impl Into<String>,
        model_sha256: impl Into<String>,
        provider_version: impl Into<String>,
        capabilities: ModelCapabilities,
    ) -> Self {
        Self {
            model_id: model_id.into(),
            model_version: model_version.into(),
            model_sha256: model_sha256.into(),
            adapter: ModelAdapterKind::QwenNative,
            adapter_version: "qwen-native-v2".to_owned(),
            tokenizer: ModelTokenizerKind::VocabularyOnly,
            tokenizer_version: format!("llama.cpp-vocab-only:{}", crate::PINNED_LLAMA_CPP_VERSION),
            provider: InferenceProviderKind::Ollama,
            provider_version: provider_version.into(),
            capabilities,
            runtime_profile: None,
        }
    }

    /// Binds the selected Qwen runner settings to prepared requests and derived caches.
    #[must_use]
    pub fn with_qwen_runtime_profile(mut self, profile: crate::QwenRuntimeProfile) -> Self {
        self.runtime_profile = Some(profile);
        self
    }

    /// Content-free effective Qwen configuration, when composed by the host.
    #[must_use]
    pub const fn qwen_runtime_profile(&self) -> Option<&crate::QwenRuntimeProfile> {
        self.runtime_profile.as_ref()
    }

    /// Digest binding persisted derived artifacts to every incompatible profile dimension.
    #[must_use]
    pub fn identity_sha256(&self) -> String {
        let mut hasher = Sha256::new();
        for component in [
            self.model_id.as_str(),
            self.model_version.as_str(),
            self.model_sha256.as_str(),
            self.adapter_version.as_str(),
            self.tokenizer_version.as_str(),
            self.provider_version.as_str(),
        ] {
            hasher.update(component.len().to_le_bytes());
            hasher.update(component.as_bytes());
        }
        if let Some(profile) = &self.runtime_profile {
            let identity = profile.identity();
            hasher.update(identity.len().to_le_bytes());
            hasher.update(identity.as_bytes());
        }
        format!("{:x}", hasher.finalize())
    }

    /// Exact application-owned model identifier.
    #[must_use]
    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    /// Exact application-owned model release.
    #[must_use]
    pub fn model_version(&self) -> &str {
        &self.model_version
    }

    /// Verified primary artifact digest.
    #[must_use]
    pub fn model_sha256(&self) -> &str {
        &self.model_sha256
    }

    /// Selected prompt/output adapter.
    #[must_use]
    pub const fn adapter(&self) -> ModelAdapterKind {
        self.adapter
    }

    /// Version of the selected adapter contract.
    #[must_use]
    pub fn adapter_version(&self) -> &str {
        &self.adapter_version
    }

    /// Selected exact-admission tokenizer.
    #[must_use]
    pub const fn tokenizer(&self) -> ModelTokenizerKind {
        self.tokenizer
    }

    /// Version of the selected tokenizer implementation.
    #[must_use]
    pub fn tokenizer_version(&self) -> &str {
        &self.tokenizer_version
    }

    /// Selected inference transport/provider.
    #[must_use]
    pub const fn provider(&self) -> InferenceProviderKind {
        self.provider
    }

    /// Version of the selected provider implementation.
    #[must_use]
    pub fn provider_version(&self) -> &str {
        &self.provider_version
    }

    /// Closed capabilities admitted for the complete profile.
    #[must_use]
    pub const fn capabilities(&self) -> &ModelCapabilities {
        &self.capabilities
    }
}

/// Why a successfully decoded generation ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GenerationCompletion {
    /// The adapter observed a complete end-of-turn boundary.
    Stop,
    /// The configured generation limit ended a partial final answer.
    Length,
}

/// Optional content-free measurements reported by an inference provider.
///
/// Every field is independently optional because providers expose different
/// counters. Callers must not derive a missing measurement from an unrelated
/// counter.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GenerationMetrics {
    /// Provider-reported prompt-token count for this generation.
    ///
    /// Some providers report the complete admitted prompt even when its KV
    /// state was reused, so this is not a cache-miss or evaluated-token count.
    pub prompt_tokens: Option<u64>,
    /// Provider-reported prompt tokens reused from a native cache.
    pub cached_prompt_tokens: Option<u64>,
    /// Provider-reported prompt tokens written to a native cache.
    pub cache_write_prompt_tokens: Option<u64>,
    /// Provider-reported generated tokens.
    pub generated_tokens: Option<u64>,
    /// Provider-reported prompt evaluation time in nanoseconds.
    pub prompt_evaluation_ns: Option<u64>,
    /// Provider-reported model generation time in nanoseconds.
    pub generation_ns: Option<u64>,
    /// Provider-reported total request time in nanoseconds.
    pub total_ns: Option<u64>,
    /// Provider-reported model loading time charged to this request.
    pub load_ns: Option<u64>,
    /// Provider-reported peak resident memory in bytes.
    pub peak_memory_bytes: Option<u64>,
}

/// One complete structured tool invocation emitted by any model adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedToolCall {
    /// Request-scoped stable identifier assigned or validated by the adapter.
    pub call_id: String,
    /// Provider channel/recipient, retained for adapter validation.
    pub recipient: String,
    /// Function name presented to the model-neutral registry.
    pub name: String,
    /// Parsed arguments in deterministic name order.
    pub arguments: BTreeMap<String, Value>,
}

/// Model-neutral output emitted by every stream decoder.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GenerationEvent {
    /// The provider delivered its first response bytes. A provider may have
    /// generated tokens before this event, particularly for structured calls.
    OutputStarted,
    /// Display-only text whose final/tool-preamble classification is unresolved.
    ContentPreviewDelta {
        /// Bounded UTF-8 fragment; never an authoritative answer or tool input.
        text: String,
    },
    /// Discard the current provisional display before reclassification or failure.
    ContentPreviewReset,
    /// Incremental private reasoning text.
    ReasoningDelta {
        /// Complete bounded UTF-8 reasoning fragment.
        text: String,
    },
    /// Incremental user-visible final content.
    FinalContentDelta {
        /// Complete bounded UTF-8 final-answer fragment.
        text: String,
    },
    /// One complete structured tool invocation.
    ToolCall {
        /// Complete validated structured invocation.
        call: ParsedToolCall,
    },
    /// Optional provider measurements with no prompt or output content.
    Metrics {
        /// Measurements exposed by the selected provider.
        metrics: GenerationMetrics,
    },
    /// The single successful terminal event.
    Completed {
        /// Sanitized provider finish reason.
        reason: GenerationCompletion,
    },
    /// The single explicit cancellation terminal event.
    Cancelled,
    /// The single non-cancellation failure terminal event.
    Failed {
        /// Closed content-free diagnostic category.
        failure: ModelOutputFailureKind,
    },
}

/// Model-neutral incremental output decoder boundary.
pub trait ModelOutputDecoder {
    /// Consumes arbitrary provider bytes and emits every complete new event.
    fn push(&mut self, bytes: &[u8]) -> Vec<GenerationEvent>;
    /// Completes the provider stream with its sanitized finish reason.
    fn finish(&mut self, reason: GenerationCompletion) -> Vec<GenerationEvent>;
    /// Emits a single cancelled terminal event when not already terminal.
    fn cancel(&mut self) -> Vec<GenerationEvent>;
    /// Emits a single upstream-failure terminal event when not already terminal.
    fn fail_upstream(&mut self) -> Vec<GenerationEvent>;
    /// Emits a single memory-pressure terminal event when not already terminal.
    fn fail_memory_pressure(&mut self) -> Vec<GenerationEvent>;
}

/// Sanitized prompt-adapter failure categories.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PromptAdapterFailureKind {
    /// Conversation roles or ordering are invalid.
    InvalidHistory,
    /// Too many tools were supplied.
    ToolLimit,
    /// Message content is empty, unsupported, or protocol-conflicting.
    InvalidContent,
    /// Model-neutral input exceeds the adapter bound.
    InputLimit,
    /// Rendered output exceeds the adapter bound.
    OutputLimit,
    /// The trusted current date is malformed.
    InvalidDate,
    /// A tool definition is malformed, duplicated, or unsafe.
    InvalidToolDefinition,
    /// Historical tool calls/results are inconsistent.
    InvalidToolCall,
    /// Deterministic serialization failed.
    Serialization,
}

/// Content-free prompt-adapter failure safe for application boundaries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PromptAdapterError {
    kind: PromptAdapterFailureKind,
}

impl PromptAdapterError {
    /// Creates a content-free adapter error.
    #[must_use]
    pub const fn new(kind: PromptAdapterFailureKind) -> Self {
        Self { kind }
    }

    /// Returns its closed diagnostic category.
    #[must_use]
    pub const fn kind(self) -> PromptAdapterFailureKind {
        self.kind
    }
}

impl fmt::Display for PromptAdapterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the model prompt could not be rendered safely")
    }
}

impl Error for PromptAdapterError {}

/// Explicit prompt rendering and output-decoder factory boundary.
pub trait ModelAdapter {
    /// Stateful decoder belonging to this exact adapter.
    type Decoder: ModelOutputDecoder;

    /// Stable adapter identity.
    fn kind(&self) -> ModelAdapterKind;
    /// Renders model-neutral history into the adapter's private wire format.
    ///
    /// # Errors
    ///
    /// Returns a content-free category when validation or rendering fails.
    fn render(&self, request: &PromptRenderRequest) -> Result<String, PromptAdapterError>;
    /// Creates one request-scoped output decoder.
    ///
    /// # Errors
    ///
    /// Returns a closed failure category when the request scope is invalid.
    fn decoder(&self, generation_id: &str) -> Result<Self::Decoder, ModelOutputFailureKind>;
}

/// Renders one model-neutral request through the selected adapter.
///
/// # Errors
///
/// Returns a sanitized error when the request violates the adapter contract.
pub fn render_model_prompt(
    adapter: ModelAdapterKind,
    request: &PromptRenderRequest,
) -> Result<String, PromptAdapterError> {
    match adapter {
        ModelAdapterKind::GlimmerAtem => GlimmerAdapter.render(request),
        ModelAdapterKind::QwenNative => Err(PromptAdapterError::new(
            PromptAdapterFailureKind::InvalidContent,
        )),
    }
}

/// Closed adapter decoder used by provider transports that carry raw model bytes.
pub struct ActiveModelDecoder(GlimmerStreamParser);

impl ActiveModelDecoder {
    /// Creates the decoder for a raw-output adapter.
    ///
    /// # Errors
    ///
    /// Returns `InvalidProtocol` when the selected adapter does not use the
    /// raw-output decoder or the generation identifier is invalid.
    pub fn new(
        adapter: ModelAdapterKind,
        generation_id: &str,
    ) -> Result<Self, ModelOutputFailureKind> {
        match adapter {
            ModelAdapterKind::GlimmerAtem => GlimmerAdapter.decoder(generation_id).map(Self),
            ModelAdapterKind::QwenNative => Err(ModelOutputFailureKind::InvalidProtocol),
        }
    }
}

impl ModelOutputDecoder for ActiveModelDecoder {
    fn push(&mut self, bytes: &[u8]) -> Vec<GenerationEvent> {
        self.0.push(bytes)
    }

    fn finish(&mut self, reason: GenerationCompletion) -> Vec<GenerationEvent> {
        self.0.finish(reason)
    }

    fn cancel(&mut self) -> Vec<GenerationEvent> {
        self.0.cancel()
    }

    fn fail_upstream(&mut self) -> Vec<GenerationEvent> {
        self.0.fail_upstream()
    }

    fn fail_memory_pressure(&mut self) -> Vec<GenerationEvent> {
        self.0.fail_memory_pressure()
    }
}

/// Model-neutral tokenizer and streaming inference-provider boundary.
pub trait InferenceProvider {
    /// Exact verified profile currently resident in the provider.
    ///
    /// # Errors
    ///
    /// Returns an error if no idle verified profile is available.
    fn active_profile(&self) -> Result<crate::ActiveInferenceProfile, crate::InferenceError>;

    /// Counts tokens through the tokenizer bound to the supplied profile.
    ///
    /// # Errors
    ///
    /// Returns an error if the binding is stale or tokenization fails.
    fn tokenize_prompt(
        &self,
        binding: &GenerationBinding,
        prompt: &str,
    ) -> Result<u64, crate::InferenceError>;

    /// Streams one generation through the provider and selected adapter.
    ///
    /// # Errors
    ///
    /// Returns an error if admission, transport, cancellation, or decoding fails.
    fn start_generation(
        &self,
        request: &crate::GenerationRequest,
        emit: &mut dyn FnMut(GenerationEvent),
    ) -> Result<(), crate::InferenceError>;
}
