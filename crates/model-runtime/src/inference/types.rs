//! Public lifecycle requests and sanitized state snapshots.

use std::path::PathBuf;

use crate::{CompanionKind, GenerationBinding, ModelCapabilities};

use super::InferenceError;

/// Exact `llama.cpp` revision built into this application.
pub const PINNED_LLAMA_CPP_VERSION: &str = "704485942ab54bbbbf1f241b3550ffba35f5f37e";
/// Fixed visual-token count produced by Glimmer's 896px, 14px-patch,
/// 2x2-spatial-merge projector.
pub const GLIMMER_IMAGE_TOKENS: u32 = 1_024;
/// Maximum images accepted by one generation and one user message.
pub const MAX_GENERATION_IMAGES: usize = 4;
const MAX_NORMALIZED_IMAGE_BYTES: usize = 20 * 1024 * 1024;
pub(super) const GLIMMER_IMAGE_MARKER: &str = "<|patch|>";

/// Rust-owned description of one managed model eligible for verification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelLoadConfig {
    pub(super) executable: PathBuf,
    pub(super) model: PathBuf,
    pub(super) expected_sha256: String,
    pub(super) expected_bytes: u64,
    pub(super) context_tokens: u32,
    pub(super) output_tokens: u32,
    pub(super) backend_compatibility_version: Option<String>,
    pub(super) companion_compatibility_version: Option<String>,
    pub(super) companions: Vec<CompanionLoadConfig>,
    pub(super) execution_profile: Option<LoadExecutionProfile>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct LoadExecutionProfile {
    pub(super) model_id: String,
    pub(super) model_version: String,
    pub(super) capabilities: ModelCapabilities,
}

/// One installed companion selected for the current inference profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompanionLoadConfig {
    pub(super) kind: CompanionKind,
    pub(super) artifact: PathBuf,
    pub(super) expected_sha256: String,
    pub(super) expected_bytes: u64,
    pub(super) compatibility_version: String,
}

impl CompanionLoadConfig {
    /// Creates a companion candidate. Full hashing occurs immediately before load.
    #[must_use]
    pub fn new(
        kind: CompanionKind,
        artifact: impl Into<PathBuf>,
        expected_sha256: impl Into<String>,
        expected_bytes: u64,
        compatibility_version: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            artifact: artifact.into(),
            expected_sha256: expected_sha256.into(),
            expected_bytes,
            compatibility_version: compatibility_version.into(),
        }
    }
}

impl ModelLoadConfig {
    /// Creates a load candidate. Full validation and hashing occur in `load_model`.
    #[must_use]
    pub fn new(
        executable: impl Into<PathBuf>,
        model: impl Into<PathBuf>,
        expected_sha256: impl Into<String>,
        expected_bytes: u64,
    ) -> Self {
        Self {
            executable: executable.into(),
            model: model.into(),
            expected_sha256: expected_sha256.into(),
            expected_bytes,
            context_tokens: 32_768,
            output_tokens: 4_096,
            backend_compatibility_version: None,
            companion_compatibility_version: None,
            companions: Vec::new(),
            execution_profile: None,
        }
    }

    /// Applies the maximum context and generated-token limits for this load.
    #[must_use]
    pub const fn with_limits(mut self, context_tokens: u32, output_tokens: u32) -> Self {
        self.context_tokens = context_tokens;
        self.output_tokens = output_tokens;
        self
    }

    /// Requires the manifest's backend compatibility revision to match this build.
    #[must_use]
    pub fn with_backend_compatibility_version(mut self, version: impl Into<String>) -> Self {
        self.backend_compatibility_version = Some(version.into());
        self
    }

    /// Selects verified companions belonging to one exact compatibility group.
    #[must_use]
    pub fn with_companions(
        mut self,
        compatibility_version: impl Into<String>,
        companions: Vec<CompanionLoadConfig>,
    ) -> Self {
        self.companion_compatibility_version = Some(compatibility_version.into());
        self.companions = companions;
        self
    }

    /// Binds catalogue identity and capabilities to every request using this load.
    #[must_use]
    pub fn with_execution_profile(
        mut self,
        model_id: impl Into<String>,
        model_version: impl Into<String>,
        capabilities: ModelCapabilities,
    ) -> Self {
        self.execution_profile = Some(LoadExecutionProfile {
            model_id: model_id.into(),
            model_version: model_version.into(),
            capabilities,
        });
        self
    }
}

/// One accepted generation using limits no greater than the loaded profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GenerationRequest {
    pub(super) id: String,
    pub(super) prompt: String,
    pub(super) output_tokens: u32,
    pub(super) images: Vec<Vec<u8>>,
    pub(super) binding: GenerationBinding,
}

impl GenerationRequest {
    /// Creates one request-scoped generation.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        prompt: impl Into<String>,
        output_tokens: u32,
        binding: GenerationBinding,
    ) -> Self {
        Self {
            id: id.into(),
            prompt: prompt.into(),
            output_tokens,
            images: Vec::new(),
            binding,
        }
    }

    /// Attaches locally validated, metadata-free PNG derivatives in prompt order.
    ///
    /// # Errors
    ///
    /// Returns an error for too many images, malformed/oversized derivatives,
    /// or a mismatch between image bytes and Glimmer image markers.
    pub fn with_normalized_images(mut self, images: Vec<Vec<u8>>) -> Result<Self, InferenceError> {
        validate_images(&self.prompt, &images)?;
        self.images = images;
        Ok(self)
    }
}

pub(super) fn validate_images(prompt: &str, images: &[Vec<u8>]) -> Result<(), InferenceError> {
    validate_normalized_image_bytes(images)?;
    if prompt.matches(GLIMMER_IMAGE_MARKER).count() != images.len() {
        return Err(InferenceError::ImageMarkerMismatch);
    }
    Ok(())
}

pub(crate) fn validate_normalized_image_bytes(images: &[Vec<u8>]) -> Result<(), InferenceError> {
    if images.len() > MAX_GENERATION_IMAGES {
        return Err(InferenceError::ImageCountExceeded);
    }
    if images.iter().any(|image| {
        image.is_empty()
            || image.len() > MAX_NORMALIZED_IMAGE_BYTES
            || !image.starts_with(b"\x89PNG\r\n\x1a\n")
    }) {
        return Err(InferenceError::InvalidImageInput);
    }
    Ok(())
}

/// Memory-pressure signal understood by the inference controller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryPressureLevel {
    /// Normal operation may proceed.
    Normal,
    /// Active generation is cancelled and new generation is refused.
    Warning,
    /// Active generation is cancelled and the model is unloaded.
    Critical,
}

/// Observable lifecycle state without paths, credentials or prompt content.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InferenceRuntimeStatus {
    /// No model is resident.
    Unloaded,
    /// A verified model is resident and idle.
    Loaded {
        /// Rust-enforced context limit.
        context_tokens: u32,
        /// Rust-enforced output limit.
        output_tokens: u32,
        /// Whether a verified vision projector is resident.
        multimodal: bool,
    },
    /// One generation is active.
    Generating {
        /// Opaque request identifier.
        generation_id: String,
    },
    /// The helper failed unexpectedly and may be restarted within its bound.
    Failed {
        /// Remaining attempts in the rolling restart window.
        restart_attempts_remaining: usize,
    },
}

/// Exact verified identity and limits of the currently loaded model profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveInferenceProfile {
    pub(super) binding: GenerationBinding,
    pub(super) context_tokens: u32,
    pub(super) output_tokens: u32,
    pub(super) image_tokens: Option<u32>,
}

impl ActiveInferenceProfile {
    /// Creates an application-verified profile for a non-`llama.cpp` provider.
    #[must_use]
    pub fn verified(
        binding: GenerationBinding,
        context_tokens: u32,
        output_tokens: u32,
        image_tokens: Option<u32>,
    ) -> Self {
        Self {
            binding,
            context_tokens,
            output_tokens,
            image_tokens,
        }
    }
    /// Verified primary model artifact digest.
    #[must_use]
    pub fn model_sha256(&self) -> &str {
        self.binding.model_sha256()
    }

    /// Immutable model, adapter, tokenizer, provider and capability binding.
    #[must_use]
    pub const fn binding(&self) -> &GenerationBinding {
        &self.binding
    }

    /// Rust-enforced context window.
    #[must_use]
    pub const fn context_tokens(&self) -> u32 {
        self.context_tokens
    }

    /// Rust-enforced maximum generated tokens.
    #[must_use]
    pub const fn output_tokens(&self) -> u32 {
        self.output_tokens
    }

    /// Fixed visual tokens per image when multimodal support is loaded.
    #[must_use]
    pub const fn image_tokens(&self) -> Option<u32> {
        self.image_tokens
    }
}
