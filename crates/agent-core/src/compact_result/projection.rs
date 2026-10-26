use serde::{Deserialize, Deserializer, Serialize, de};

use crate::OpaqueLocalReference;

use super::{CompactResultError, CompactResultStatus};

const MAX_EXCERPT_BYTES: usize = 256;

/// One deliberately small excerpt approved for model-visible context.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ModelVisibleExcerpt(String);

impl ModelVisibleExcerpt {
    /// Validates one selected excerpt at the trusted host boundary.
    ///
    /// This conservative gate rejects private-root shapes and common authority,
    /// secret and diagnostic markers. Callers should omit an excerpt whenever
    /// its sensitivity is uncertain.
    ///
    /// # Errors
    ///
    /// Returns an error for empty, oversized, control-bearing, path-like or
    /// potentially sensitive content.
    pub fn new(value: impl Into<String>) -> Result<Self, CompactResultError> {
        let value = value.into();
        let lower = value.to_ascii_lowercase();
        let sensitive_markers = [
            "api_key",
            "api-key",
            "access_token",
            "access-token",
            "authorization:",
            "-----begin",
            "bearer ",
            "bookmark",
            "diagnostic",
            "ghp_",
            "github_pat_",
            "password",
            "private key",
            "recovery phrase",
            "secret",
            "seed phrase",
            "sk-",
            "xoxb-",
        ];
        if value.is_empty()
            || value.len() > MAX_EXCERPT_BYTES
            || value.chars().any(char::is_control)
            || value.starts_with('/')
            || value.starts_with('~')
            || lower.contains("file://")
            || lower.contains("/users/")
            || lower.contains("/home/")
            || lower.contains("/private/")
            || lower.contains("\\users\\")
            || looks_like_drive_root(&lower)
            || contains_long_credential_candidate(&value)
            || sensitive_markers
                .iter()
                .any(|marker| lower.contains(marker))
        {
            return Err(CompactResultError::UnsafeModelProjection);
        }
        Ok(Self(value))
    }

    /// Returns the bounded excerpt.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(super) fn validate(&self) -> Result<(), CompactResultError> {
        Self::new(self.0.clone()).map(|_| ())
    }
}

impl<'de> Deserialize<'de> for ModelVisibleExcerpt {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(|_| de::Error::custom("unsafe model-visible excerpt"))
    }
}

/// Authority-free source metadata safe for model-visible history.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CompactModelSource {
    /// A workspace-relative source reference.
    Workspace {
        /// Model-facing alias, not a private root.
        workspace_alias: String,
        /// Validated relative path.
        relative_path: String,
        /// Observed resource revision.
        resource_revision: String,
        /// Observed content digest.
        content_sha256: String,
    },
    /// An opaque managed-asset source reference.
    ManagedAsset {
        /// Opaque asset ID.
        asset_id: String,
        /// Exact asset revision.
        resource_revision: String,
        /// Validated content digest.
        content_sha256: String,
    },
    /// An opaque retained permissioned external response.
    ExternalResponse {
        /// Opaque response ID, never its URL.
        response_id: String,
        /// Digest of the exact canonical request and policy.
        request_sha256: String,
        /// Digest of the retained response representation.
        response_sha256: String,
        /// Whether retained content may be reopened without a fresh request.
        immutable: bool,
    },
}

/// Bounded authority-free compact record safe for prompt adapters.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactToolResultProjection {
    /// Opaque local reference used for an explicit reopen request.
    pub reference: OpaqueLocalReference,
    /// Exact historical tool call association.
    pub tool_call_id: String,
    /// Trusted tool registry name.
    pub tool_name: String,
    /// Explicit result status.
    pub status: CompactResultStatus,
    /// Host-generated bounded summary containing no retained payload.
    pub summary: String,
    /// Digest of the complete retained payload.
    pub payload_sha256: String,
    /// Serialized retained byte count.
    pub payload_bytes: u64,
    /// Tool-specific item count.
    pub item_count: u64,
    /// Safe source references with no capability handles or private roots.
    pub sources: Vec<CompactModelSource>,
    /// Explicitly selected and conservatively validated excerpts.
    pub excerpts: Vec<ModelVisibleExcerpt>,
}

fn looks_like_drive_root(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.windows(3).any(|window| {
        window[0].is_ascii_alphabetic() && window[1] == b':' && matches!(window[2], b'/' | b'\\')
    })
}

fn contains_long_credential_candidate(value: &str) -> bool {
    value
        .split(|character: char| !(character.is_ascii_alphanumeric() || "_-".contains(character)))
        .any(|candidate| {
            candidate.len() >= 32
                && candidate.bytes().any(|byte| byte.is_ascii_alphabetic())
                && candidate.bytes().any(|byte| byte.is_ascii_digit())
        })
}
