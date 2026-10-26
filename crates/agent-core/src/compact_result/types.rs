use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ToolOutcome;

/// Terminal status retained without copying the original result into history.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CompactResultStatus {
    /// The tool completed.
    Completed,
    /// The tool failed with a typed failure.
    Failed,
    /// The tool was cancelled.
    Cancelled,
}

impl CompactResultStatus {
    pub(super) const fn from_outcome(outcome: &ToolOutcome) -> Self {
        match outcome {
            ToolOutcome::Completed { .. } => Self::Completed,
            ToolOutcome::Failed { .. } => Self::Failed,
            ToolOutcome::Cancelled { .. } => Self::Cancelled,
        }
    }
}

/// Large content retained only in trusted local persistence.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RetainedToolPayload {
    /// UTF-8 tool output.
    Text {
        /// Complete retained text.
        text: String,
    },
    /// Structured tool output.
    Json {
        /// Complete retained JSON.
        value: Value,
    },
}

/// Typed compact-result validation and rehydration outcomes.
#[derive(Debug)]
pub enum CompactResultError {
    /// Record structure or trusted provenance is malformed.
    InvalidRecord,
    /// Retained content exceeds its local persistence ceiling.
    PayloadTooLarge,
    /// Retained bytes do not match their persisted digest or byte count.
    DigestMismatch,
    /// Model-visible content failed conservative sensitivity validation.
    UnsafeModelProjection,
    /// The explicit reopen used another project or conversation.
    ScopeMismatch,
    /// A capability was revoked/changed or a source/asset revision is unavailable.
    SourceChangedOrUnavailable,
    /// The retention deadline elapsed.
    Expired,
    /// JSON serialization or decoding failed.
    Serialization(serde_json::Error),
}

impl CompactResultError {
    /// Returns whether a fresh tool read or graceful final response can recover.
    #[must_use]
    pub const fn is_recoverable(&self) -> bool {
        matches!(
            self,
            Self::ScopeMismatch | Self::SourceChangedOrUnavailable | Self::Expired
        )
    }
}

impl fmt::Display for CompactResultError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialization(_) => formatter.write_str("compact result serialization failed"),
            other => write!(formatter, "{other:?}"),
        }
    }
}

impl Error for CompactResultError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Serialization(error) => Some(error),
            _ => None,
        }
    }
}

impl From<serde_json::Error> for CompactResultError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialization(error)
    }
}

impl From<crate::ExecutionLedgerError> for CompactResultError {
    fn from(_: crate::ExecutionLedgerError) -> Self {
        Self::InvalidRecord
    }
}

impl From<crate::ToolRegistryError> for CompactResultError {
    fn from(_: crate::ToolRegistryError) -> Self {
        Self::InvalidRecord
    }
}
