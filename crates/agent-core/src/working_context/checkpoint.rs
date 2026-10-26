use serde::{Deserialize, Serialize};

use super::{UntrustedWorkingContextSummary, WorkingContextError};

const CHECKPOINT_VERSION: u16 = 1;
const MAX_CHECKPOINT_BYTES: usize = 16 * 1024;

/// Persistable untrusted semantic summary bound to exact covered history and host policy.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SemanticCompactionCheckpoint {
    version: u16,
    conversation_id: String,
    project_id: String,
    covered_message_count: u32,
    covered_message_id: String,
    history_sha256: String,
    source_state_sha256: String,
    summary_policy_version: u16,
    #[serde(alias = "modelSha256")]
    binding_sha256: String,
    summary: UntrustedWorkingContextSummary,
    updated_at: String,
}

impl SemanticCompactionCheckpoint {
    /// Creates a checkpoint containing no ledger, permission or approval state.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed scope, digests, bounds or summary data.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        conversation_id: impl Into<String>,
        project_id: impl Into<String>,
        covered_message_count: u32,
        covered_message_id: impl Into<String>,
        history_sha256: impl Into<String>,
        source_state_sha256: impl Into<String>,
        summary_policy_version: u16,
        binding_sha256: impl Into<String>,
        summary: UntrustedWorkingContextSummary,
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
            summary_policy_version,
            binding_sha256: binding_sha256.into(),
            summary,
            updated_at: updated_at.into(),
        };
        checkpoint.validate()?;
        Ok(checkpoint)
    }

    /// Validates a reconstructed checkpoint and its complete bounded encoding.
    ///
    /// # Errors
    ///
    /// Returns an error when stored checkpoint data is malformed or unsupported.
    pub fn validate(&self) -> Result<(), WorkingContextError> {
        if self.version != CHECKPOINT_VERSION
            || !valid_id(&self.conversation_id)
            || !valid_id(&self.project_id)
            || self.covered_message_count == 0
            || !valid_id(&self.covered_message_id)
            || !valid_digest(&self.history_sha256)
            || !valid_digest(&self.source_state_sha256)
            || self.summary_policy_version == 0
            || !valid_digest(&self.binding_sha256)
            || self.updated_at.is_empty()
            || self.updated_at.len() > 64
            || self.summary.validate().is_err()
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

    /// Number of complete leading messages covered by the summary.
    #[must_use]
    pub const fn covered_message_count(&self) -> u32 {
        self.covered_message_count
    }

    /// Last exact message covered by the summary.
    #[must_use]
    pub fn covered_message_id(&self) -> &str {
        &self.covered_message_id
    }

    /// Digest of the exact serialized covered message prefix.
    #[must_use]
    pub fn history_sha256(&self) -> &str {
        &self.history_sha256
    }

    /// Digest of source, permission and ledger assumptions at checkpoint creation.
    /// Historical metadata only; live evidence must validate its own source bindings.
    #[must_use]
    pub fn source_state_sha256(&self) -> &str {
        &self.source_state_sha256
    }

    /// Summary contract version.
    #[must_use]
    pub const fn summary_policy_version(&self) -> u16 {
        self.summary_policy_version
    }

    /// Exact model/adapter/tokenizer/provider binding digest used for the summary.
    #[must_use]
    pub fn binding_sha256(&self) -> &str {
        &self.binding_sha256
    }

    /// Validated but untrusted semantic summary.
    #[must_use]
    pub const fn summary(&self) -> &UntrustedWorkingContextSummary {
        &self.summary
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
