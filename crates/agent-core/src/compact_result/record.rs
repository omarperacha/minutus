use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    OpaqueLocalReference, OpaqueReferenceKind, ToolResultEnvelope, ledger_types::validate_id,
};

use super::{
    CompactRehydrationContext, CompactResultError, CompactResultStatus,
    CompactToolResultProjection, ComputationProvenanceRecord, HostSourceBinding,
    ModelVisibleExcerpt, RetainedToolPayload,
    source::{is_safe_opaque_id, validate_sources},
};

const RECORD_VERSION: u16 = 1;
const MAX_RETAINED_BYTES: usize = 16 * 1024 * 1024;
const MAX_EXCERPTS: usize = 4;

/// Complete retained record. This type is never an IPC or prompt value.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompactToolResultRecord {
    version: u16,
    id: String,
    conversation_id: String,
    project_id: String,
    tool_call_id: String,
    tool_name: String,
    status: CompactResultStatus,
    payload: RetainedToolPayload,
    payload_sha256: String,
    payload_bytes: u64,
    item_count: u64,
    sources: Vec<HostSourceBinding>,
    excerpts: Vec<ModelVisibleExcerpt>,
    created_at: String,
    expires_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    computation_provenance: Option<ComputationProvenanceRecord>,
}

impl CompactToolResultRecord {
    /// Creates one retention-bounded record from an exact terminal tool result.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed scope, payload, provenance, excerpts or
    /// timestamps, including a payload over the local retention ceiling.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        conversation_id: impl Into<String>,
        project_id: impl Into<String>,
        result: &ToolResultEnvelope,
        payload: RetainedToolPayload,
        item_count: u64,
        mut sources: Vec<HostSourceBinding>,
        excerpts: Vec<ModelVisibleExcerpt>,
        created_at: impl Into<String>,
        expires_at: impl Into<String>,
    ) -> Result<Self, CompactResultError> {
        validate_sources(&mut sources)?;
        let payload_bytes = serde_json::to_vec(&payload)?;
        let payload_size =
            u64::try_from(payload_bytes.len()).map_err(|_| CompactResultError::PayloadTooLarge)?;
        let payload_sha256 = format!("{:x}", Sha256::digest(&payload_bytes));
        let record = Self {
            version: RECORD_VERSION,
            id: id.into(),
            conversation_id: conversation_id.into(),
            project_id: project_id.into(),
            tool_call_id: result.call_id().to_owned(),
            tool_name: result.tool_name().to_owned(),
            status: CompactResultStatus::from_outcome(result.outcome()),
            payload,
            payload_sha256,
            payload_bytes: payload_size,
            item_count,
            sources,
            excerpts,
            created_at: created_at.into(),
            expires_at: expires_at.into(),
            computation_provenance: None,
        };
        record.validate()?;
        Ok(record)
    }

    /// Attaches validated host-only computation provenance before persistence.
    ///
    /// # Errors
    ///
    /// Returns an error when the provenance or resulting retained record is invalid.
    pub fn with_computation_provenance(
        mut self,
        provenance: ComputationProvenanceRecord,
    ) -> Result<Self, CompactResultError> {
        provenance.validate()?;
        self.computation_provenance = Some(provenance);
        self.validate()?;
        Ok(self)
    }

    /// Validates a decoded persisted record and its content digest.
    ///
    /// # Errors
    ///
    /// Returns an error when persisted content or provenance was corrupted.
    pub fn validate(&self) -> Result<(), CompactResultError> {
        if self.version != RECORD_VERSION {
            return Err(CompactResultError::InvalidRecord);
        }
        for (label, value) in [
            ("compact result ID", self.id.as_str()),
            ("compact conversation ID", self.conversation_id.as_str()),
            ("compact project ID", self.project_id.as_str()),
            ("compact tool call ID", self.tool_call_id.as_str()),
            ("compact tool name", self.tool_name.as_str()),
        ] {
            validate_id(label, value)?;
        }
        if !is_safe_opaque_id(&self.id) {
            return Err(CompactResultError::InvalidRecord);
        }
        validate_timestamp(&self.created_at)?;
        validate_timestamp(&self.expires_at)?;
        if self.created_at >= self.expires_at
            || self.excerpts.len() > MAX_EXCERPTS
            || self
                .excerpts
                .iter()
                .any(|excerpt| excerpt.validate().is_err())
        {
            return Err(CompactResultError::InvalidRecord);
        }
        let mut sources = self.sources.clone();
        validate_sources(&mut sources)?;
        if sources != self.sources {
            return Err(CompactResultError::InvalidRecord);
        }
        let payload = serde_json::to_vec(&self.payload)?;
        if payload.is_empty() || payload.len() > MAX_RETAINED_BYTES {
            return Err(CompactResultError::PayloadTooLarge);
        }
        let payload_bytes =
            u64::try_from(payload.len()).map_err(|_| CompactResultError::PayloadTooLarge)?;
        let digest = format!("{:x}", Sha256::digest(&payload));
        if payload_bytes != self.payload_bytes || digest != self.payload_sha256 {
            return Err(CompactResultError::DigestMismatch);
        }
        if let Some(provenance) = &self.computation_provenance {
            provenance.validate()?;
            let expected_status = match provenance.terminal_status() {
                super::ComputationTerminalStatus::Completed => CompactResultStatus::Completed,
                super::ComputationTerminalStatus::Cancelled => CompactResultStatus::Cancelled,
                _ => CompactResultStatus::Failed,
            };
            let mut provenance_sources = provenance.source_bindings().cloned().collect::<Vec<_>>();
            provenance_sources.sort();
            if expected_status != self.status || provenance_sources != self.sources {
                return Err(CompactResultError::InvalidRecord);
            }
        }
        Ok(())
    }

    /// Returns the opaque persistence key.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the owning conversation.
    #[must_use]
    pub fn conversation_id(&self) -> &str {
        &self.conversation_id
    }

    /// Returns the owning project security scope.
    #[must_use]
    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    /// Returns the exact originating tool call.
    #[must_use]
    pub fn tool_call_id(&self) -> &str {
        &self.tool_call_id
    }

    /// Returns the trusted registry tool name.
    #[must_use]
    pub fn tool_name(&self) -> &str {
        &self.tool_name
    }

    /// Returns the terminal result status.
    #[must_use]
    pub const fn status(&self) -> CompactResultStatus {
        self.status
    }

    /// Returns the retention deadline.
    #[must_use]
    pub fn expires_at(&self) -> &str {
        &self.expires_at
    }

    /// Returns the creation timestamp used by retention persistence.
    #[must_use]
    pub fn created_at(&self) -> &str {
        &self.created_at
    }

    /// Returns managed assets kept live by this record.
    #[must_use]
    pub fn asset_ids(&self) -> Vec<&str> {
        self.sources
            .iter()
            .filter_map(HostSourceBinding::asset_id)
            .collect()
    }

    /// Returns complete host-only source bindings for live broker revalidation.
    #[must_use]
    pub fn source_bindings(&self) -> &[HostSourceBinding] {
        &self.sources
    }

    /// Returns host-only computation provenance when this is a computation result.
    #[must_use]
    pub const fn computation_provenance(&self) -> Option<&ComputationProvenanceRecord> {
        self.computation_provenance.as_ref()
    }

    /// Builds the only projection intended for model-visible history.
    ///
    /// # Errors
    ///
    /// Returns an error if the stored record no longer validates.
    pub fn model_projection(&self) -> Result<CompactToolResultProjection, CompactResultError> {
        self.validate()?;
        Ok(CompactToolResultProjection {
            reference: OpaqueLocalReference::new(
                self.id.clone(),
                OpaqueReferenceKind::CompactToolResult,
            )?,
            tool_call_id: self.tool_call_id.clone(),
            tool_name: self.tool_name.clone(),
            status: self.status,
            summary: format!(
                "{:?} tool result retained locally ({} bytes, {} items, {} sources).",
                self.status,
                self.payload_bytes,
                self.item_count,
                self.sources.len()
            ),
            payload_sha256: self.payload_sha256.clone(),
            payload_bytes: self.payload_bytes,
            item_count: self.item_count,
            sources: self
                .sources
                .iter()
                .map(HostSourceBinding::model_projection)
                .collect(),
            excerpts: self.excerpts.clone(),
        })
    }

    /// Returns retained content only after exact live scope and revision checks.
    ///
    /// # Errors
    ///
    /// Returns a typed recoverable outcome for expiry, scope changes, revoked
    /// capabilities, missing assets or changed source revisions.
    pub fn rehydrate(
        &self,
        context: &CompactRehydrationContext,
        now: &str,
    ) -> Result<&RetainedToolPayload, CompactResultError> {
        self.validate()?;
        validate_timestamp(now)?;
        if now >= self.expires_at.as_str() {
            return Err(CompactResultError::Expired);
        }
        if context.project_id != self.project_id || context.conversation_id != self.conversation_id
        {
            return Err(CompactResultError::ScopeMismatch);
        }
        if context.sources != self.sources {
            return Err(CompactResultError::SourceChangedOrUnavailable);
        }
        if self.sources.iter().any(|source| {
            matches!(
                source,
                HostSourceBinding::ExternalResponse {
                    response_id,
                    immutable: false,
                    ..
                } if !context.freshly_retrieved_external_responses.contains(response_id)
            )
        }) {
            return Err(CompactResultError::SourceChangedOrUnavailable);
        }
        Ok(&self.payload)
    }
}

fn validate_timestamp(value: &str) -> Result<(), CompactResultError> {
    let bytes = value.as_bytes();
    let structure = value.len() == 20
        && bytes.get(4) == Some(&b'-')
        && bytes.get(7) == Some(&b'-')
        && bytes.get(10) == Some(&b'T')
        && bytes.get(13) == Some(&b':')
        && bytes.get(16) == Some(&b':')
        && bytes.get(19) == Some(&b'Z')
        && bytes.iter().enumerate().all(|(index, byte)| {
            matches!(index, 4 | 7 | 10 | 13 | 16 | 19) || byte.is_ascii_digit()
        });
    if !structure
        || decimal_pair(bytes, 5).is_none_or(|month| !(1..=12).contains(&month))
        || decimal_pair(bytes, 8).is_none_or(|day| !(1..=31).contains(&day))
        || decimal_pair(bytes, 11).is_none_or(|hour| hour > 23)
        || decimal_pair(bytes, 14).is_none_or(|minute| minute > 59)
        || decimal_pair(bytes, 17).is_none_or(|second| second > 59)
    {
        return Err(CompactResultError::InvalidRecord);
    }
    Ok(())
}

fn decimal_pair(bytes: &[u8], start: usize) -> Option<u8> {
    let tens = bytes.get(start)?.checked_sub(b'0')?;
    let ones = bytes.get(start + 1)?.checked_sub(b'0')?;
    tens.checked_mul(10)?.checked_add(ones)
}
