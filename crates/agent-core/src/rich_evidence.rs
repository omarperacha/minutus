//! Persistable untrusted evidence extracted from expensive rich inputs.

use serde::{Deserialize, Serialize};

use crate::WorkingContextError;

const CHECKPOINT_VERSION: u16 = 1;
const MAX_CHECKPOINT_BYTES: usize = 16 * 1024;
const MAX_EXTRACTION_BYTES: usize = 12 * 1024;
const MAX_SOURCES: usize = 8;

/// Opaque provenance for one immutable managed rich-input derivative.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RichEvidenceSource {
    asset_id: String,
    content_sha256: String,
}

impl RichEvidenceSource {
    /// Creates one validated opaque source reference.
    ///
    /// # Errors
    ///
    /// Returns an error for a malformed asset identifier or digest.
    pub fn new(
        asset_id: impl Into<String>,
        content_sha256: impl Into<String>,
    ) -> Result<Self, WorkingContextError> {
        let source = Self {
            asset_id: asset_id.into(),
            content_sha256: content_sha256.into(),
        };
        source.validate()?;
        Ok(source)
    }

    /// Opaque managed asset identifier.
    #[must_use]
    pub fn asset_id(&self) -> &str {
        &self.asset_id
    }

    /// Digest of the exact normalized inference bytes.
    #[must_use]
    pub fn content_sha256(&self) -> &str {
        &self.content_sha256
    }

    fn validate(&self) -> Result<(), WorkingContextError> {
        if !valid_id(&self.asset_id) || !valid_digest(&self.content_sha256) {
            return Err(WorkingContextError::InvalidCheckpoint);
        }
        Ok(())
    }
}

/// Untrusted model extraction bound to exact history, sources, policy and model adapter.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RichEvidenceCheckpoint {
    version: u16,
    conversation_id: String,
    project_id: String,
    covered_message_count: u32,
    covered_message_id: String,
    history_sha256: String,
    source_state_sha256: String,
    extraction_policy_version: u16,
    binding_sha256: String,
    sources: Vec<RichEvidenceSource>,
    extraction: String,
    updated_at: String,
}

impl RichEvidenceCheckpoint {
    /// Creates a checkpoint containing no ledger facts, permissions, outcomes or approvals.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed scope, provenance, bounds or extraction data.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        conversation_id: impl Into<String>,
        project_id: impl Into<String>,
        covered_message_count: u32,
        covered_message_id: impl Into<String>,
        history_sha256: impl Into<String>,
        source_state_sha256: impl Into<String>,
        extraction_policy_version: u16,
        binding_sha256: impl Into<String>,
        sources: Vec<RichEvidenceSource>,
        extraction: impl Into<String>,
        updated_at: impl Into<String>,
    ) -> Result<Self, WorkingContextError> {
        let checkpoint = Self {
            version: CHECKPOINT_VERSION,
            conversation_id: conversation_id.into(),
            project_id: project_id.into(),
            covered_message_count,
            covered_message_id: covered_message_id.into(),
            history_sha256: history_sha256.into(),
            source_state_sha256: source_state_sha256.into(),
            extraction_policy_version,
            binding_sha256: binding_sha256.into(),
            sources,
            extraction: extraction.into(),
            updated_at: updated_at.into(),
        };
        checkpoint.validate()?;
        Ok(checkpoint)
    }

    /// Validates reconstructed checkpoint data and its complete bounded encoding.
    ///
    /// # Errors
    ///
    /// Returns an error when stored checkpoint data is malformed or unsupported.
    pub fn validate(&self) -> Result<(), WorkingContextError> {
        let sources_valid = !self.sources.is_empty()
            && self.sources.len() <= MAX_SOURCES
            && self.sources.iter().all(|source| source.validate().is_ok())
            && self
                .sources
                .windows(2)
                .all(|pair| pair[0].asset_id < pair[1].asset_id);
        if self.version != CHECKPOINT_VERSION
            || !valid_id(&self.conversation_id)
            || !valid_id(&self.project_id)
            || self.covered_message_count == 0
            || !valid_id(&self.covered_message_id)
            || !valid_digest(&self.history_sha256)
            || !valid_digest(&self.source_state_sha256)
            || self.extraction_policy_version == 0
            || !valid_digest(&self.binding_sha256)
            || !sources_valid
            || self.extraction.trim().is_empty()
            || self.extraction.len() > MAX_EXTRACTION_BYTES
            || self.updated_at.is_empty()
            || self.updated_at.len() > 64
            || serde_json::to_vec(self).map_or(true, |encoded| encoded.len() > MAX_CHECKPOINT_BYTES)
        {
            return Err(WorkingContextError::InvalidCheckpoint);
        }
        Ok(())
    }

    /// Owning conversation scope.
    #[must_use]
    pub fn conversation_id(&self) -> &str {
        &self.conversation_id
    }

    /// Owning project scope.
    #[must_use]
    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    /// Number of complete leading messages covered by the extraction.
    #[must_use]
    pub const fn covered_message_count(&self) -> u32 {
        self.covered_message_count
    }

    /// Last exact message covered by the extraction.
    #[must_use]
    pub fn covered_message_id(&self) -> &str {
        &self.covered_message_id
    }

    /// Digest of the exact serialized covered message prefix.
    #[must_use]
    pub fn history_sha256(&self) -> &str {
        &self.history_sha256
    }

    /// Digest of current trusted workspace and execution-ledger assumptions.
    #[must_use]
    pub fn source_state_sha256(&self) -> &str {
        &self.source_state_sha256
    }

    /// Host extraction-policy version.
    #[must_use]
    pub const fn extraction_policy_version(&self) -> u16 {
        self.extraction_policy_version
    }

    /// Exact model, adapter, tokenizer and provider binding digest.
    #[must_use]
    pub fn binding_sha256(&self) -> &str {
        &self.binding_sha256
    }

    /// Exact immutable managed sources in stable identifier order.
    #[must_use]
    pub fn sources(&self) -> &[RichEvidenceSource] {
        &self.sources
    }

    /// Bounded model-authored extraction, always treated as untrusted content.
    #[must_use]
    pub fn extraction(&self) -> &str {
        &self.extraction
    }

    /// Last checkpoint update timestamp.
    #[must_use]
    pub fn updated_at(&self) -> &str {
        &self.updated_at
    }
}

fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 160 && !value.chars().any(char::is_whitespace)
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checkpoint(extraction: &str) -> Result<RichEvidenceCheckpoint, WorkingContextError> {
        RichEvidenceCheckpoint::new(
            "conversation",
            "project",
            2,
            "message",
            "a".repeat(64),
            "d".repeat(64),
            1,
            "b".repeat(64),
            vec![RichEvidenceSource::new("asset", "c".repeat(64))?],
            extraction,
            "2026-08-23T00:00:00Z",
        )
    }

    #[test]
    fn checkpoint_is_bounded_and_has_no_authority_fields() -> Result<(), Box<dyn std::error::Error>>
    {
        let captured = checkpoint("The chart shows two labeled columns.")?;
        let value = serde_json::to_value(&captured)?;
        assert_eq!(captured.sources()[0].asset_id(), "asset");
        for forbidden in ["ledger", "permission", "approval", "outcome", "capability"] {
            assert!(value.get(forbidden).is_none());
        }
        assert!(checkpoint(&"x".repeat(MAX_EXTRACTION_BYTES + 1)).is_err());
        assert!(checkpoint("   ").is_err());
        Ok(())
    }

    #[test]
    fn sources_must_be_unique_and_stably_ordered() -> Result<(), Box<dyn std::error::Error>> {
        let source = RichEvidenceSource::new("asset", "c".repeat(64))?;
        let duplicate = RichEvidenceCheckpoint::new(
            "conversation",
            "project",
            2,
            "message",
            "a".repeat(64),
            "d".repeat(64),
            1,
            "b".repeat(64),
            vec![source.clone(), source],
            "facts",
            "2026-08-23T00:00:00Z",
        );
        assert!(duplicate.is_err());
        Ok(())
    }
}
