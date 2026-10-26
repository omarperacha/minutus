//! Stable runner settings for the measured Qwen bundle. No prompt-dependent policy.

use super::{OllamaChatRequest, VerifiedOllamaModel, WireBool};

pub(crate) const OLLAMA_PARALLEL_CONTEXTS: u16 = 1;

// Qualification does not advance automatically when the bundled runtime is upgraded.
const QUALIFIED_OLLAMA_VERSION: &str = "0.32.14";
const QWEN_MANIFEST: &str = "22130167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643";

/// Startup-only selection. The host must establish hardware and single-slot ownership.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum QwenTuningPolicy {
    /// Preserve manifest options; also the known-good rollback.
    #[default]
    Existing,
    /// M1 Pro, 32 GiB, pinned Ollama, one parallel context. No speedup claimed.
    M1ProBaselineV1,
}

/// Immutable, content-free runner identity shared by admission and generation.
/// The verified manifest binds weights, projector and manifest-supplied options.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QwenRuntimeProfile {
    manifest_sha256: String,
    context_tokens: u32,
    policy: QwenTuningPolicy,
}

impl QwenRuntimeProfile {
    /// Stable schema/profile identifier; existing means no tuning qualification.
    #[must_use]
    pub const fn id(&self) -> &'static str {
        match self.policy {
            QwenTuningPolicy::Existing => "qwen-existing-v1",
            QwenTuningPolicy::M1ProBaselineV1 => "qwen-m1-pro-32gb-mtp4-v1",
        }
    }

    /// Whether the explicitly measured baseline settings apply.
    #[must_use]
    pub const fn is_measured_baseline(&self) -> bool {
        matches!(self.policy, QwenTuningPolicy::M1ProBaselineV1)
    }

    /// Context window shared by preflight and generation.
    #[must_use]
    pub const fn context_tokens(&self) -> u32 {
        self.context_tokens
    }

    /// Explicit draft depth; `None` preserves the verified manifest's setting.
    #[must_use]
    pub fn draft_tokens(&self) -> Option<u32> {
        self.is_measured_baseline().then_some(4)
    }

    /// Required parallel contexts for the measured baseline; external fallback is unknown.
    #[must_use]
    pub fn parallel_contexts(&self) -> Option<u16> {
        self.is_measured_baseline()
            .then_some(OLLAMA_PARALLEL_CONTEXTS)
    }

    /// Native chat retains the longest reusable prefix in the resident slot.
    #[must_use]
    pub const fn cache_prompt(&self) -> bool {
        true
    }

    /// Verified manifest binding the primary model, projector and inherited options.
    #[must_use]
    pub fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }

    pub(crate) fn identity(&self) -> String {
        format!(
            "{}:{}:{}",
            self.id(),
            self.manifest_sha256,
            self.context_tokens
        )
    }

    pub(crate) fn apply(&self, request: &mut OllamaChatRequest) {
        if let Some(options) = &mut request.options {
            options.num_ctx = self.context_tokens;
            options.draft_num_predict = self.draft_tokens();
        }
        request.shift = WireBool(self.cache_prompt());
    }
}

impl VerifiedOllamaModel {
    /// Resolves settings only for the exact measured bundle, backend and context.
    /// Unqualified combinations retain manifest-supplied options.
    #[must_use]
    pub fn runtime_profile(&self, context_tokens: u32) -> QwenRuntimeProfile {
        let measured = self.tuning_policy == QwenTuningPolicy::M1ProBaselineV1
            && self.tag == "qwen3.8:27b"
            && self.manifest_sha256 == QWEN_MANIFEST
            && self.provider_version == QUALIFIED_OLLAMA_VERSION
            && context_tokens == 32_768;
        QwenRuntimeProfile {
            manifest_sha256: self.manifest_sha256.clone(),
            context_tokens,
            policy: if measured {
                QwenTuningPolicy::M1ProBaselineV1
            } else {
                QwenTuningPolicy::Existing
            },
        }
    }
}

impl OllamaChatRequest {
    pub(super) fn render_only(&self) -> Self {
        let mut request = self.clone();
        request.stream = false;
        request.debug_render_only = true;
        request
    }
}

#[cfg(test)]
mod tests;

#[cfg(all(test, target_os = "macos"))]
mod live_tests;
