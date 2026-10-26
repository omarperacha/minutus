//! Local model lifecycle and model-neutral generation events.

pub mod asr;
mod blocking_client;
mod catalogue;
mod download;
mod external;
mod generation;
mod gguf;
mod glimmer;
mod import;
mod inference;
mod ollama;
mod ollama_sidecar;
mod output_failure;
mod prompt;
mod qwen;
mod sidecar;
mod tokenizer;

pub use catalogue::{
    ArtifactFormat, ArtifactKey, CATALOGUE_SCHEMA_VERSION, CatalogueError, CompanionArtifact,
    CompanionKind, InferenceBackend, ModelArtifact, ModelCapabilities, ModelCapability,
    ModelCatalogue, ModelDefaults, ModelIntegrationStatus, ModelKey, ModelManifest, ModelPlatform,
    MultimodalProfile, OllamaBlob, OllamaDistribution, SpeechBackend, SpeechCatalogue,
    SpeechProfile, bundled_catalogue, bundled_speech_catalogue,
};
pub use download::{
    DownloadCancellation, DownloadError, DownloadPhase, DownloadProgress, ModelDownloadManager,
    VerifiedModelArtifact,
};
pub use generation::{
    ActiveModelDecoder, GenerationBinding, GenerationCompletion, GenerationEvent,
    GenerationMetrics, InferenceProvider, InferenceProviderKind, ModelAdapter, ModelAdapterKind,
    ModelOutputDecoder, ModelTokenizerKind, ParsedToolCall, PromptAdapterError,
    PromptAdapterFailureKind, render_model_prompt,
};
pub use gguf::{GgufError, GgufMetadata, inspect_gguf};
pub use import::{ImportError, ImportProgress, ImportedModelArtifact, ModelImportManager};
pub use inference::{
    ActiveInferenceProfile, CompanionLoadConfig, GLIMMER_IMAGE_TOKENS, GenerationRequest,
    InferenceError, InferenceRuntimeStatus, LocalInferenceRuntime, MAX_GENERATION_IMAGES,
    MemoryPressureLevel, ModelLoadConfig, PINNED_LLAMA_CPP_VERSION,
};
pub use ollama::{
    OllamaChatMessage, OllamaChatRequest, OllamaError, OllamaGenerateRequest, OllamaProvider,
    OllamaProviderConfig, OllamaStreamChunk, QwenRuntimeProfile, QwenTuningPolicy,
    VerifiedOllamaModel,
};
pub use ollama_sidecar::{
    OLLAMA_GUARDIAN_ARGUMENT, OllamaSidecar, OllamaSidecarConfig, OllamaSidecarEndpoint,
    OllamaSidecarError, OllamaSidecarStatus, PINNED_OLLAMA_VERSION, run_ollama_guardian,
};
pub use output_failure::ModelOutputFailureKind;
pub use prompt::{
    PromptContent, PromptMessage, PromptRenderRequest, PromptToolCall, PromptToolDefinition,
    ReasoningStrength,
};
pub use qwen::{QwenAdapter, QwenStreamDecoder, QwenStreamError};

pub use sidecar::{
    ApiCredential, LlamaSidecar, LlamaSidecarConfig, MultimodalSidecarConfig, SidecarEndpoint,
    SidecarError, SidecarStatus,
};
pub use tokenizer::{ExactTokenizer, TokenizerError};

pub use external::{
    ExternalFileVerification, ExternalModelError, external_file_stamp, link_external_blob,
    register_external_glimmer_metadata, register_external_qwen_metadata, unlink_external_blob,
    unregister_external_model_metadata, verify_external_file,
};
