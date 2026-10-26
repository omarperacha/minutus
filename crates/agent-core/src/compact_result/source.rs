use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::ledger_types::{validate_alias, validate_digest, validate_id, validate_relative_path};

use super::{CompactModelSource, CompactResultError};

const MAX_SOURCES: usize = 32;

/// Host-only source and capability assumptions for retained content.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum HostSourceBinding {
    /// A capability-scoped workspace file or directory.
    Workspace {
        /// Model-facing workspace alias.
        workspace_alias: String,
        /// Validated relative resource path.
        relative_path: String,
        /// Opaque durable capability identity, never model-visible.
        capability_id: String,
        /// Exact durable capability revision, never model-visible.
        capability_revision: String,
        /// Exact observed resource revision.
        resource_revision: String,
        /// Digest of the observed content.
        content_sha256: String,
    },
    /// A managed asset whose bytes remain in the asset store.
    ManagedAsset {
        /// Opaque managed asset identifier.
        asset_id: String,
        /// Exact asset revision.
        resource_revision: String,
        /// Digest of the validated asset bytes.
        content_sha256: String,
    },
    /// One bounded permissioned external response retained outside history.
    ExternalResponse {
        /// Opaque response identity with no URL semantics.
        response_id: String,
        /// Digest of the exact canonical request and broker policy.
        request_sha256: String,
        /// Digest of the complete retained response representation.
        response_sha256: String,
        /// Whether policy permits retained-content rehydration without refetching.
        immutable: bool,
    },
}

impl HostSourceBinding {
    /// Creates a workspace source without retaining an absolute root or bookmark.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed aliases, paths, IDs, revisions or digests.
    pub fn workspace(
        workspace_alias: impl Into<String>,
        relative_path: impl Into<String>,
        capability_id: impl Into<String>,
        capability_revision: impl Into<String>,
        resource_revision: impl Into<String>,
        content_sha256: impl Into<String>,
    ) -> Result<Self, CompactResultError> {
        let source = Self::Workspace {
            workspace_alias: workspace_alias.into(),
            relative_path: relative_path.into(),
            capability_id: capability_id.into(),
            capability_revision: capability_revision.into(),
            resource_revision: resource_revision.into(),
            content_sha256: content_sha256.into(),
        };
        source.validate()?;
        Ok(source)
    }

    /// Creates a managed-asset source using only opaque identity and revision data.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed IDs, revisions or digests.
    pub fn managed_asset(
        asset_id: impl Into<String>,
        resource_revision: impl Into<String>,
        content_sha256: impl Into<String>,
    ) -> Result<Self, CompactResultError> {
        let source = Self::ManagedAsset {
            asset_id: asset_id.into(),
            resource_revision: resource_revision.into(),
            content_sha256: content_sha256.into(),
        };
        source.validate()?;
        Ok(source)
    }

    /// Creates a permissioned external-response source binding.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed opaque identity or request/response digests.
    pub fn external_response(
        response_id: impl Into<String>,
        request_sha256: impl Into<String>,
        response_sha256: impl Into<String>,
        immutable: bool,
    ) -> Result<Self, CompactResultError> {
        let source = Self::ExternalResponse {
            response_id: response_id.into(),
            request_sha256: request_sha256.into(),
            response_sha256: response_sha256.into(),
            immutable,
        };
        source.validate()?;
        Ok(source)
    }

    /// Returns the managed asset ID when this binding retains one.
    #[must_use]
    pub fn asset_id(&self) -> Option<&str> {
        match self {
            Self::Workspace { .. } | Self::ExternalResponse { .. } => None,
            Self::ManagedAsset { asset_id, .. } => Some(asset_id),
        }
    }

    pub(super) fn validate(&self) -> Result<(), CompactResultError> {
        match self {
            Self::Workspace {
                workspace_alias,
                relative_path,
                capability_id,
                capability_revision,
                resource_revision,
                content_sha256,
            } => {
                validate_alias(workspace_alias)?;
                validate_relative_path(relative_path)?;
                validate_id("compact capability ID", capability_id)?;
                validate_id("compact capability revision", capability_revision)?;
                validate_id("compact resource revision", resource_revision)?;
                validate_digest(content_sha256)?;
            }
            Self::ManagedAsset {
                asset_id,
                resource_revision,
                content_sha256,
            } => {
                validate_id("compact asset ID", asset_id)?;
                if !is_safe_opaque_id(asset_id) {
                    return Err(CompactResultError::InvalidRecord);
                }
                validate_id("compact asset revision", resource_revision)?;
                validate_digest(content_sha256)?;
            }
            Self::ExternalResponse {
                response_id,
                request_sha256,
                response_sha256,
                ..
            } => {
                validate_id("compact external response ID", response_id)?;
                if !is_safe_opaque_id(response_id) {
                    return Err(CompactResultError::InvalidRecord);
                }
                validate_digest(request_sha256)?;
                validate_digest(response_sha256)?;
            }
        }
        Ok(())
    }

    pub(super) fn model_projection(&self) -> CompactModelSource {
        match self {
            Self::Workspace {
                workspace_alias,
                relative_path,
                resource_revision,
                content_sha256,
                ..
            } => CompactModelSource::Workspace {
                workspace_alias: workspace_alias.clone(),
                relative_path: relative_path.clone(),
                resource_revision: resource_revision.clone(),
                content_sha256: content_sha256.clone(),
            },
            Self::ManagedAsset {
                asset_id,
                resource_revision,
                content_sha256,
            } => CompactModelSource::ManagedAsset {
                asset_id: asset_id.clone(),
                resource_revision: resource_revision.clone(),
                content_sha256: content_sha256.clone(),
            },
            Self::ExternalResponse {
                response_id,
                request_sha256,
                response_sha256,
                immutable,
            } => CompactModelSource::ExternalResponse {
                response_id: response_id.clone(),
                request_sha256: request_sha256.clone(),
                response_sha256: response_sha256.clone(),
                immutable: *immutable,
            },
        }
    }
}

/// Current trusted scope supplied when explicitly reopening retained content.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompactRehydrationContext {
    pub(super) project_id: String,
    pub(super) conversation_id: String,
    pub(super) sources: Vec<HostSourceBinding>,
    pub(super) freshly_retrieved_external_responses: BTreeSet<String>,
}

impl CompactRehydrationContext {
    /// Creates current capability and revision evidence for one explicit reopen.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed scope, duplicate sources or excessive input.
    pub fn new(
        project_id: impl Into<String>,
        conversation_id: impl Into<String>,
        sources: Vec<HostSourceBinding>,
    ) -> Result<Self, CompactResultError> {
        let mut context = Self {
            project_id: project_id.into(),
            conversation_id: conversation_id.into(),
            sources,
            freshly_retrieved_external_responses: BTreeSet::new(),
        };
        validate_id("rehydration project ID", &context.project_id)?;
        validate_id("rehydration conversation ID", &context.conversation_id)?;
        validate_sources(&mut context.sources)?;
        Ok(context)
    }

    /// Creates live evidence after the named external responses were freshly fetched.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed, duplicate or unbound response IDs.
    pub fn after_fresh_external_retrieval(
        project_id: impl Into<String>,
        conversation_id: impl Into<String>,
        sources: Vec<HostSourceBinding>,
        response_ids: impl IntoIterator<Item = String>,
    ) -> Result<Self, CompactResultError> {
        let mut context = Self::new(project_id, conversation_id, sources)?;
        for response_id in response_ids {
            validate_id("fresh external response ID", &response_id)?;
            if !is_safe_opaque_id(&response_id)
                || !context
                    .freshly_retrieved_external_responses
                    .insert(response_id.clone())
                || !context.sources.iter().any(|source| {
                    matches!(source, HostSourceBinding::ExternalResponse { response_id: bound, .. } if bound == &response_id)
                })
            {
                return Err(CompactResultError::InvalidRecord);
            }
        }
        Ok(context)
    }
}

pub(super) fn validate_sources(
    sources: &mut Vec<HostSourceBinding>,
) -> Result<(), CompactResultError> {
    if sources.len() > MAX_SOURCES {
        return Err(CompactResultError::InvalidRecord);
    }
    for source in &*sources {
        source.validate()?;
    }
    sources.sort();
    let unique = sources.iter().collect::<BTreeSet<_>>();
    if unique.len() != sources.len() {
        return Err(CompactResultError::InvalidRecord);
    }
    Ok(())
}

pub(super) fn is_safe_opaque_id(value: &str) -> bool {
    !value.contains('/') && !value.contains('\\') && !value.contains(':')
}
