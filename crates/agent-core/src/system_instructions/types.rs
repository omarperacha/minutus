use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

/// Current revision of Minutus's host-authored behavioral policy.
pub const AGENT_SYSTEM_POLICY_VERSION: u16 = 5;
pub(crate) const FLOWZED_KNOWLEDGE_BUNDLE_VERSION: u16 = 3;

/// Persistable identity of the exact trusted instruction content used by a run.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSystemInstructionIdentity {
    policy_version: u16,
    knowledge_bundle_version: Option<u16>,
    content_sha256: String,
}

impl AgentSystemInstructionIdentity {
    pub(crate) fn new(
        policy_version: u16,
        knowledge_bundle_version: Option<u16>,
        content_sha256: String,
    ) -> Self {
        Self {
            policy_version,
            knowledge_bundle_version,
            content_sha256,
        }
    }

    /// Returns the host-authored behavioral policy revision.
    #[must_use]
    pub const fn policy_version(&self) -> u16 {
        self.policy_version
    }

    /// Returns the admitted local `FlowZed` bundle revision, when relevant.
    #[must_use]
    pub const fn knowledge_bundle_version(&self) -> Option<u16> {
        self.knowledge_bundle_version
    }

    /// Returns the digest of the complete exact system instruction text.
    #[must_use]
    pub fn content_sha256(&self) -> &str {
        &self.content_sha256
    }

    pub(crate) fn validate(&self) -> Result<(), AgentSystemInstructionError> {
        if self.policy_version != AGENT_SYSTEM_POLICY_VERSION
            || self
                .knowledge_bundle_version
                .is_some_and(|version| version != FLOWZED_KNOWLEDGE_BUNDLE_VERSION)
            || self.content_sha256.len() != 64
            || !self
                .content_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(AgentSystemInstructionError::InvalidInstructionIdentity);
        }
        Ok(())
    }
}

/// Safe provenance for one locally bundled knowledge source.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowZedKnowledgeSource {
    pub(crate) url: &'static str,
    pub(crate) source_reviewed_on: &'static str,
}

impl FlowZedKnowledgeSource {
    /// Returns the canonical first-party source URL.
    #[must_use]
    pub const fn url(self) -> &'static str {
        self.url
    }

    /// Returns the date this bundled source was reviewed through live retrieval.
    #[must_use]
    pub const fn source_reviewed_on(self) -> &'static str {
        self.source_reviewed_on
    }
}

/// Exact model-neutral system content produced by trusted host code.
///
/// Only its authority-free identity is serializable. The complete prompt text
/// is deliberately unavailable through ordinary JSON, audit or IPC projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentSystemInstructions {
    identity: AgentSystemInstructionIdentity,
    content: String,
    knowledge_sources: &'static [FlowZedKnowledgeSource],
}

impl AgentSystemInstructions {
    pub(crate) fn new(
        identity: AgentSystemInstructionIdentity,
        content: String,
        knowledge_sources: &'static [FlowZedKnowledgeSource],
    ) -> Self {
        Self {
            identity,
            content,
            knowledge_sources,
        }
    }

    /// Returns the exact trusted text for the model adapter's system role.
    #[must_use]
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Returns the safe identity that persisted run state should retain.
    #[must_use]
    pub const fn identity(&self) -> &AgentSystemInstructionIdentity {
        &self.identity
    }

    /// Returns admitted first-party knowledge provenance, separately from text.
    #[must_use]
    pub const fn knowledge_sources(&self) -> &[FlowZedKnowledgeSource] {
        self.knowledge_sources
    }
}

/// Failure while assembling trusted system instructions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentSystemInstructionError {
    /// Current date is not strict `YYYY-MM-DD` host metadata.
    InvalidCurrentDate,
    /// A compiled local knowledge resource no longer matches its pinned digest.
    InvalidKnowledgeBundle,
    /// Trusted content contains an active model-adapter boundary marker.
    ReservedDelimiter,
    /// Complete instructions exceed the product-owned byte ceiling.
    InstructionsTooLarge,
    /// Untrusted selection text exceeds the bounded admission scan.
    SelectionTooLarge,
    /// Persisted instruction version or digest is malformed or unsupported.
    InvalidInstructionIdentity,
}

impl fmt::Display for AgentSystemInstructionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidCurrentDate => "trusted system instruction date is invalid",
            Self::InvalidKnowledgeBundle => "bundled FlowZed knowledge failed verification",
            Self::ReservedDelimiter => "trusted system instructions contain a reserved delimiter",
            Self::InstructionsTooLarge => "trusted system instructions exceed their size limit",
            Self::SelectionTooLarge => "knowledge selection text exceeds its size limit",
            Self::InvalidInstructionIdentity => "trusted system instruction identity is invalid",
        };
        formatter.write_str(message)
    }
}

impl Error for AgentSystemInstructionError {}
