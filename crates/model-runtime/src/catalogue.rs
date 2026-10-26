//! Strict, versioned catalogue for publisher-provided model artifacts.

use std::{collections::HashSet, error::Error, fmt};

use serde::Deserialize;
use url::Url;

mod platform;
mod speech;
mod tensor_bundle;
mod validation;

pub use platform::ModelPlatform;
pub use tensor_bundle::OllamaBlob;

pub use speech::{SpeechBackend, SpeechCatalogue, SpeechProfile, bundled_speech_catalogue};

mod error;

pub use error::CatalogueError;
use validation::{RawCatalogue, validate_model};

/// Catalogue schema understood by this build.
pub const CATALOGUE_SCHEMA_VERSION: u32 = 4;

const MAX_CATALOGUE_BYTES: usize = 1024 * 1024;
const MAX_MODELS: usize = 128;
const MAX_COMPANIONS: usize = 8;
const MAX_PROFILE_IMAGES: u32 = 4;
const MAX_ID_BYTES: usize = 128;
const MAX_VERSION_BYTES: usize = 128;
const MAX_DISPLAY_NAME_BYTES: usize = 256;
const MAX_FILENAME_BYTES: usize = 255;
const MAX_CONTEXT_TOKENS: u32 = 1_048_576;

/// Parses and validates the catalogue embedded in the application binary.
///
/// # Errors
///
/// Returns an error if the bundled document is malformed or violates an invariant.
pub fn bundled_catalogue() -> Result<ModelCatalogue, CatalogueError> {
    ModelCatalogue::parse_json(include_str!("../catalogue/v4.json"))
}

/// Stable identity of one model release.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ModelKey {
    id: String,
    version: String,
}

impl ModelKey {
    /// Returns the publisher-qualified model identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the immutable model revision.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }
}

impl fmt::Display for ModelKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}@{}", self.id, self.version)
    }
}

/// Stable identity of one artifact belonging to a model release.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ArtifactKey {
    model: ModelKey,
    id: String,
}

impl ArtifactKey {
    /// Returns the owning model release.
    #[must_use]
    pub const fn model(&self) -> &ModelKey {
        &self.model
    }

    /// Returns the model-release-local artifact identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }
}

impl fmt::Display for ArtifactKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}#{}", self.model, self.id)
    }
}

/// Closed set of model artifact formats understood by the runtime.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactFormat {
    /// `llama.cpp`'s GGUF model container.
    Gguf,
    /// Provider-owned safetensors tensor shards, with a pinned complete manifest.
    Safetensors,
    /// Whisper’s native ggml weights, distinct from GGUF.
    #[serde(rename = "whisper-ggml")]
    WhisperGgml,
}

impl ArtifactFormat {
    /// Returns the stable lowercase storage representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Gguf => "gguf",
            Self::Safetensors => "safetensors",
            Self::WhisperGgml => "whisper-ggml",
        }
    }
}

/// Closed set of inference backends supported by catalogue entries.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum InferenceBackend {
    /// The bundled `llama.cpp` server.
    LlamaCpp,
    /// A separately installed, loopback-only Ollama server.
    Ollama,
}

/// Whether this build may offer a validated profile for installation/use.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum ModelIntegrationStatus {
    /// The provider and adapter are implemented by this build.
    Integrated,
    /// The provider lifecycle is implemented, while the model adapter is pending.
    ProviderReady,
    /// Identity is pinned for comparative work, but execution is not implemented.
    BaselineOnly,
}

/// Closed model-neutral capabilities used by every supported model profile.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum ModelCapability {
    /// Native text-message input.
    TextInput,
    /// Native image input.
    ImageInput,
    /// Structured function definitions and calls.
    StructuredToolCalls,
    /// Private reasoning output separate from final content.
    SeparateReasoningOutput,
}

/// Closed model-neutral capability set and maximum context window.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelCapabilities {
    features: Vec<ModelCapability>,
    context_limit_tokens: u32,
}

impl ModelCapabilities {
    pub(crate) fn glimmer_runtime(context_limit_tokens: u32, image_input: bool) -> Self {
        let mut features = vec![
            ModelCapability::TextInput,
            ModelCapability::StructuredToolCalls,
            ModelCapability::SeparateReasoningOutput,
        ];
        if image_input {
            features.push(ModelCapability::ImageInput);
        }
        Self {
            features,
            context_limit_tokens,
        }
    }

    /// Whether the native profile accepts text messages.
    #[must_use]
    pub fn native_text_input(&self) -> bool {
        self.features.contains(&ModelCapability::TextInput)
    }

    /// Whether images are native provider inputs rather than text encodings.
    #[must_use]
    pub fn native_image_input(&self) -> bool {
        self.features.contains(&ModelCapability::ImageInput)
    }

    /// Whether the native protocol can request structured function calls.
    #[must_use]
    pub fn structured_tool_calls(&self) -> bool {
        self.features
            .contains(&ModelCapability::StructuredToolCalls)
    }

    /// Whether private reasoning is emitted separately from final content.
    #[must_use]
    pub fn separate_reasoning_output(&self) -> bool {
        self.features
            .contains(&ModelCapability::SeparateReasoningOutput)
    }

    /// Maximum context window claimed by the pinned distribution.
    #[must_use]
    pub const fn context_limit_tokens(&self) -> u32 {
        self.context_limit_tokens
    }
}

/// Immutable Ollama distribution identity for a baseline or integrated profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OllamaDistribution {
    tensor_bundle: Option<tensor_bundle::TensorBundle>,
    tag: String,
    manifest_sha256: String,
    config_sha256: String,
    config_bytes: u64,
    parameters_sha256: String,
    parameters_bytes: u64,
    license_sha256: String,
    license_bytes: u64,
    renderer: String,
    parser: String,
}

impl OllamaDistribution {
    /// Complete blob identities for a provider-owned MLX bundle.
    #[must_use]
    pub fn tensor_blobs(&self) -> Option<&[OllamaBlob]> {
        self.tensor_bundle
            .as_ref()
            .map(|bundle| bundle.blobs.as_slice())
    }

    /// Pinned native tokenizer, when weights do not embed a GGUF vocabulary.
    #[must_use]
    pub fn tokenizer_blob(&self) -> Option<&OllamaBlob> {
        self.tensor_bundle.as_ref().map(|bundle| &bundle.tokenizer)
    }

    /// Total bytes of a complete tensor distribution, including its metadata.
    #[must_use]
    pub fn tensor_bundle_bytes(&self) -> Option<u64> {
        self.tensor_bundle.as_ref().map(|bundle| bundle.total_bytes)
    }
    /// Exact user-facing Ollama model tag.
    #[must_use]
    pub fn tag(&self) -> &str {
        &self.tag
    }
    /// SHA-256 of the canonical Ollama manifest bytes.
    #[must_use]
    pub fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }
    /// SHA-256 of the Ollama configuration blob.
    #[must_use]
    pub fn config_sha256(&self) -> &str {
        &self.config_sha256
    }
    /// Exact configuration-blob size.
    #[must_use]
    pub const fn config_bytes(&self) -> u64 {
        self.config_bytes
    }
    /// SHA-256 of the generation-parameter blob.
    #[must_use]
    pub fn parameters_sha256(&self) -> &str {
        &self.parameters_sha256
    }
    /// Exact generation-parameter-blob size.
    #[must_use]
    pub const fn parameters_bytes(&self) -> u64 {
        self.parameters_bytes
    }
    /// SHA-256 of the bundled licence blob.
    #[must_use]
    pub fn license_sha256(&self) -> &str {
        &self.license_sha256
    }
    /// Exact bundled-licence-blob size.
    #[must_use]
    pub const fn license_bytes(&self) -> u64 {
        self.license_bytes
    }
    /// Ollama renderer selected by the manifest configuration.
    #[must_use]
    pub fn renderer(&self) -> &str {
        &self.renderer
    }
    /// Ollama output parser selected by the manifest configuration.
    #[must_use]
    pub fn parser(&self) -> &str {
        &self.parser
    }
}

/// Closed set of optional model companion roles understood by the runtime.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum CompanionKind {
    /// Multimodal perception projector required when an image profile is selected.
    VisionProjector,
    /// Speculative-decoding model that proposes tokens for main-model verification.
    DraftModel,
}

impl CompanionKind {
    /// Returns the stable camel-case persistence representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::VisionProjector => "visionProjector",
            Self::DraftModel => "draftModel",
        }
    }
}

/// Product defaults applied when starting a model release.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelDefaults {
    context_tokens: u32,
    output_tokens: u32,
}

impl ModelDefaults {
    /// Returns the default total context window.
    #[must_use]
    pub const fn context_tokens(self) -> u32 {
        self.context_tokens
    }

    /// Returns the default maximum generated tokens.
    #[must_use]
    pub const fn output_tokens(self) -> u32 {
        self.output_tokens
    }
}

/// One validated downloadable primary model artifact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelArtifact {
    key: ArtifactKey,
    file_name: String,
    format: ArtifactFormat,
    download_url: Url,
    download_bytes: u64,
    installed_bytes: u64,
    sha256: String,
}

impl ModelArtifact {
    /// Returns the globally unique artifact identity.
    #[must_use]
    pub const fn key(&self) -> &ArtifactKey {
        &self.key
    }

    /// Returns the safe managed-storage filename.
    #[must_use]
    pub fn file_name(&self) -> &str {
        &self.file_name
    }

    /// Returns the on-disk format.
    #[must_use]
    pub const fn format(&self) -> ArtifactFormat {
        self.format
    }

    /// Returns the immutable HTTPS download URL.
    #[must_use]
    pub const fn download_url(&self) -> &Url {
        &self.download_url
    }

    /// Returns the expected number of transferred bytes.
    #[must_use]
    pub const fn download_bytes(&self) -> u64 {
        self.download_bytes
    }

    /// Returns the expected installed size in bytes.
    #[must_use]
    pub const fn installed_bytes(&self) -> u64 {
        self.installed_bytes
    }

    /// Returns the expected lowercase SHA-256 digest.
    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}

/// One validated companion artifact associated with a primary model release.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompanionArtifact {
    artifact: ModelArtifact,
    kind: CompanionKind,
    required: bool,
    compatibility_version: String,
    memory_requirement_bytes: u64,
}

/// Model-specific limits for one verified vision-projector profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MultimodalProfile {
    projector_artifact_id: Option<String>,
    image_tokens: u32,
    max_images: u32,
}

impl MultimodalProfile {
    /// Returns an external projector identity; embedded vision has no companion.
    #[must_use]
    pub fn projector_artifact_id(&self) -> Option<&str> {
        self.projector_artifact_id.as_deref()
    }

    /// Returns the fixed number of context tokens reserved for each image.
    #[must_use]
    pub const fn image_tokens(&self) -> u32 {
        self.image_tokens
    }

    /// Returns the maximum number of images accepted by one generation.
    #[must_use]
    pub const fn max_images(&self) -> u32 {
        self.max_images
    }
}

impl CompanionArtifact {
    /// Returns the independently downloadable and verifiable artifact.
    #[must_use]
    pub const fn artifact(&self) -> &ModelArtifact {
        &self.artifact
    }

    /// Returns the companion's runtime role.
    #[must_use]
    pub const fn kind(&self) -> CompanionKind {
        self.kind
    }

    /// Reports whether every profile for this model release requires the artifact.
    #[must_use]
    pub const fn required(&self) -> bool {
        self.required
    }

    /// Reports whether setup must provide this module before the model is usable.
    /// Image projectors are mandatory setup assets, but need not be loaded by a
    /// text-only runtime profile. Drafts follow the release's runtime requirement.
    #[must_use]
    pub const fn required_for_setup(&self) -> bool {
        self.required || matches!(self.kind, CompanionKind::VisionProjector)
    }

    /// Returns the exact model/companion compatibility group.
    #[must_use]
    pub fn compatibility_version(&self) -> &str {
        &self.compatibility_version
    }

    /// Returns the additional recommended unified memory for this companion.
    #[must_use]
    pub const fn memory_requirement_bytes(&self) -> u64 {
        self.memory_requirement_bytes
    }
}

/// One validated model release available to the application.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelManifest {
    key: ModelKey,
    display_name: String,
    artifact: ModelArtifact,
    companions: Vec<CompanionArtifact>,
    companion_compatibility_version: String,
    multimodal_profile: Option<MultimodalProfile>,
    recommended_memory_bytes: u64,
    defaults: ModelDefaults,
    integration_status: ModelIntegrationStatus,
    capabilities: ModelCapabilities,
    backend: InferenceBackend,
    backend_compatibility_version: String,
    ollama: Option<OllamaDistribution>,
    license_url: Url,
    usage_policy_url: Url,
}

impl ModelManifest {
    /// Returns the stable model-release identity.
    #[must_use]
    pub const fn key(&self) -> &ModelKey {
        &self.key
    }

    /// Returns the user-visible model name.
    #[must_use]
    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    /// Returns the primary downloadable artifact.
    #[must_use]
    pub const fn artifact(&self) -> &ModelArtifact {
        &self.artifact
    }

    /// Returns companion artifacts in stable catalogue order.
    #[must_use]
    pub fn companions(&self) -> &[CompanionArtifact] {
        &self.companions
    }

    /// Resolves one model-local companion artifact identifier.
    #[must_use]
    pub fn companion(&self, artifact_id: &str) -> Option<&CompanionArtifact> {
        self.companions
            .iter()
            .find(|companion| companion.artifact().key().id() == artifact_id)
    }

    /// Returns the compatibility group required by every selected companion.
    #[must_use]
    pub fn companion_compatibility_version(&self) -> &str {
        &self.companion_compatibility_version
    }

    /// Returns the verified projector profile declared for this release.
    #[must_use]
    pub const fn multimodal_profile(&self) -> Option<&MultimodalProfile> {
        self.multimodal_profile.as_ref()
    }

    /// Returns the recommended unified memory in bytes.
    #[must_use]
    pub const fn recommended_memory_bytes(&self) -> u64 {
        self.recommended_memory_bytes
    }

    /// Returns product-owned context and output defaults.
    #[must_use]
    pub const fn defaults(&self) -> ModelDefaults {
        self.defaults
    }

    /// Returns whether the profile is integrated or recorded only as a baseline.
    #[must_use]
    pub const fn integration_status(&self) -> ModelIntegrationStatus {
        self.integration_status
    }

    /// Reports whether this build may install and execute the profile.
    #[must_use]
    pub fn is_integrated(&self) -> bool {
        self.supports_current_runtime()
            && matches!(self.integration_status, ModelIntegrationStatus::Integrated)
    }

    /// Reports whether this build can explicitly install or register the profile.
    #[must_use]
    pub fn is_manageable(&self) -> bool {
        self.supports_current_runtime()
            && matches!(
                self.integration_status,
                ModelIntegrationStatus::Integrated | ModelIntegrationStatus::ProviderReady
            )
    }

    /// Returns the model-neutral capability profile.
    #[must_use]
    pub const fn capabilities(&self) -> &ModelCapabilities {
        &self.capabilities
    }

    /// Returns the required inference backend.
    #[must_use]
    pub const fn backend(&self) -> InferenceBackend {
        self.backend
    }

    /// Returns the exact backend revision tested with this model release.
    #[must_use]
    pub fn backend_compatibility_version(&self) -> &str {
        &self.backend_compatibility_version
    }

    /// Returns immutable Ollama distribution metadata when applicable.
    #[must_use]
    pub const fn ollama_distribution(&self) -> Option<&OllamaDistribution> {
        self.ollama.as_ref()
    }

    /// Returns the publisher's licence document.
    #[must_use]
    pub const fn license_url(&self) -> &Url {
        &self.license_url
    }

    /// Returns the publisher's usage-policy document.
    #[must_use]
    pub const fn usage_policy_url(&self) -> &Url {
        &self.usage_policy_url
    }
}

mod model_catalogue;

pub use model_catalogue::ModelCatalogue;

#[cfg(test)]
mod tests;
