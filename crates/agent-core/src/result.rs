use std::{collections::BTreeSet, fmt};

use serde::Serialize;
use serde_json::Value;

use crate::{error::ToolRegistryError, request::ValidatedToolRequest};

const MAX_REFERENCE_ID_BYTES: usize = 160;
const MAX_FAILURE_CODE_BYTES: usize = 80;
const MAX_FAILURE_MESSAGE_BYTES: usize = 1_024;

/// Result content kinds an executor is trusted to return.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SupportedResultType {
    /// Bounded UTF-8 text.
    Text,
    /// Bounded structured JSON.
    Json,
    /// An opaque reference to host-owned local data.
    OpaqueLocalReference,
    /// A validated managed asset, such as an FS-008 image derivative.
    ValidatedAsset,
}

/// Host-owned categories for opaque local references.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OpaqueReferenceKind {
    /// A retained tool result held outside model-visible history.
    CompactToolResult,
    /// A retained filesystem change or diff.
    Change,
    /// A transaction proposal held by the trusted host.
    TransactionProposal,
    /// A bounded remote response retained outside conversation JSON.
    ExternalResponse,
}

/// A local reference that deliberately carries no path, bytes, or authority.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpaqueLocalReference {
    id: String,
    kind: OpaqueReferenceKind,
}

impl OpaqueLocalReference {
    /// Creates a bounded opaque reference.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty, oversized, path-like, or whitespace-bearing ID.
    pub fn new(
        id: impl Into<String>,
        kind: OpaqueReferenceKind,
    ) -> Result<Self, ToolRegistryError> {
        let id = id.into();
        validate_opaque_id(&id)?;
        Ok(Self { id, kind })
    }

    /// Returns the non-authority-bearing identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the trusted reference category.
    #[must_use]
    pub const fn kind(&self) -> OpaqueReferenceKind {
        self.kind
    }
}

/// Media types accepted for normalized image assets.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum ValidatedImageMediaType {
    /// Metadata-free normalized PNG.
    #[serde(rename = "image/png")]
    Png,
    /// Validated static JPEG.
    #[serde(rename = "image/jpeg")]
    Jpeg,
    /// Validated static WebP.
    #[serde(rename = "image/webp")]
    WebP,
}

/// Safe metadata for an already-ingested managed image.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidatedAssetReference {
    asset_id: String,
    media_type: ValidatedImageMediaType,
    byte_size: u64,
    width: u32,
    height: u32,
    content_sha256: String,
}

impl ValidatedAssetReference {
    /// Creates a reference to bytes already validated and owned by the asset store.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid opaque ID, digest, byte size, or dimensions.
    pub fn image(
        asset_id: impl Into<String>,
        media_type: ValidatedImageMediaType,
        byte_size: u64,
        width: u32,
        height: u32,
        content_sha256: impl Into<String>,
    ) -> Result<Self, ToolRegistryError> {
        let asset_id = asset_id.into();
        let content_sha256 = content_sha256.into();
        validate_opaque_id(&asset_id)?;
        if byte_size == 0 || width == 0 || height == 0 {
            return Err(ToolRegistryError::InvalidResult(
                "validated asset dimensions and byte size must be non-zero".to_owned(),
            ));
        }
        if !is_lower_hex_digest(&content_sha256) {
            return Err(ToolRegistryError::InvalidResult(
                "validated asset digest is invalid".to_owned(),
            ));
        }
        Ok(Self {
            asset_id,
            media_type,
            byte_size,
            width,
            height,
            content_sha256,
        })
    }

    /// Returns the opaque managed-asset ID.
    #[must_use]
    pub fn asset_id(&self) -> &str {
        &self.asset_id
    }
}

/// One content part in a completed result.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ResultContent {
    /// UTF-8 text, bounded with the complete envelope.
    Text {
        /// Tool-produced text.
        text: String,
    },
    /// Structured JSON, bounded with the complete envelope.
    Json {
        /// Tool-produced structured value.
        value: Value,
    },
    /// Reference to local host-owned state.
    OpaqueLocalReference {
        /// Non-authority-bearing local reference.
        reference: OpaqueLocalReference,
    },
    /// Reference to a validated managed asset; never raw bytes.
    ValidatedAsset {
        /// Safe metadata for the managed asset.
        asset: ValidatedAssetReference,
    },
}

impl ResultContent {
    pub(crate) const fn result_type(&self) -> SupportedResultType {
        match self {
            Self::Text { .. } => SupportedResultType::Text,
            Self::Json { .. } => SupportedResultType::Json,
            Self::OpaqueLocalReference { .. } => SupportedResultType::OpaqueLocalReference,
            Self::ValidatedAsset { .. } => SupportedResultType::ValidatedAsset,
        }
    }
}

/// Sanitized failure information safe for logs and model context.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolFailure {
    code: String,
    message: String,
    transient: bool,
}

impl ToolFailure {
    /// Creates a bounded sanitized failure.
    ///
    /// # Errors
    ///
    /// Returns an error when the code or user-safe message is malformed or oversized.
    pub fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        transient: bool,
    ) -> Result<Self, ToolRegistryError> {
        let code = code.into();
        let message = message.into();
        if code.is_empty()
            || code.len() > MAX_FAILURE_CODE_BYTES
            || !code
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(ToolRegistryError::InvalidResult(
                "failure code is invalid".to_owned(),
            ));
        }
        if message.is_empty()
            || message.len() > MAX_FAILURE_MESSAGE_BYTES
            || message.chars().any(char::is_control)
        {
            return Err(ToolRegistryError::InvalidResult(
                "failure message is invalid".to_owned(),
            ));
        }
        Ok(Self {
            code,
            message,
            transient,
        })
    }

    /// Returns the stable sanitized failure code.
    #[must_use]
    pub fn code(&self) -> &str {
        &self.code
    }

    /// Returns the bounded user-safe failure message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Returns whether an unchanged call may be transiently retried.
    #[must_use]
    pub const fn transient(&self) -> bool {
        self.transient
    }

    /// Returns whether trusted policy permits a bounded retry.
    #[must_use]
    pub const fn is_transient(&self) -> bool {
        self.transient
    }
}

/// Trusted reasons a tool invocation can be cancelled.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CancellationReason {
    /// The user stopped the task.
    UserRequested,
    /// The tool exceeded its trusted deadline.
    DeadlineExceeded,
    /// Required permission was withdrawn while pending or executing.
    PermissionWithdrawn,
    /// The application stopped before work completed.
    HostShutdown,
}

/// Explicit terminal outcome for one tool request.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ToolOutcome {
    /// Execution completed with bounded typed content.
    Completed {
        /// Ordered result parts.
        content: Vec<ResultContent>,
    },
    /// Execution failed with sanitized details.
    Failed {
        /// Sanitized failure details.
        failure: ToolFailure,
    },
    /// Execution did not complete and produced no authoritative success.
    Cancelled {
        /// Trusted cancellation reason.
        reason: CancellationReason,
    },
}

/// One bounded terminal envelope associated with an exact validated request.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultEnvelope {
    call_id: String,
    tool_name: String,
    outcome: ToolOutcome,
}

impl ToolResultEnvelope {
    pub(crate) fn completed(
        request: &ValidatedToolRequest,
        content: Vec<ResultContent>,
    ) -> Result<Self, ToolRegistryError> {
        if content.len() > request.limits().max_result_parts {
            return Err(ToolRegistryError::ResultLimitExceeded);
        }
        let supported = request.supported_results();
        if content
            .iter()
            .any(|part| !supported.contains(&part.result_type()))
        {
            return Err(ToolRegistryError::UnsupportedResultType);
        }
        Self::bounded(request, ToolOutcome::Completed { content })
    }

    pub(crate) fn failed(
        request: &ValidatedToolRequest,
        failure: ToolFailure,
    ) -> Result<Self, ToolRegistryError> {
        Self::bounded(request, ToolOutcome::Failed { failure })
    }

    pub(crate) fn cancelled(
        request: &ValidatedToolRequest,
        reason: CancellationReason,
    ) -> Result<Self, ToolRegistryError> {
        Self::bounded(request, ToolOutcome::Cancelled { reason })
    }

    fn bounded(
        request: &ValidatedToolRequest,
        outcome: ToolOutcome,
    ) -> Result<Self, ToolRegistryError> {
        let result = Self {
            call_id: request.call_id().to_owned(),
            tool_name: request.tool_name().to_owned(),
            outcome,
        };
        let size = serde_json::to_vec(&result)
            .map_err(|error| ToolRegistryError::InvalidResult(error.to_string()))?
            .len();
        if size > request.limits().max_output_bytes {
            return Err(ToolRegistryError::ResultLimitExceeded);
        }
        Ok(result)
    }

    /// Returns the exact model adapter call ID.
    #[must_use]
    pub fn call_id(&self) -> &str {
        &self.call_id
    }

    /// Returns the trusted registry tool name.
    #[must_use]
    pub fn tool_name(&self) -> &str {
        &self.tool_name
    }

    /// Returns the explicit terminal outcome.
    #[must_use]
    pub const fn outcome(&self) -> &ToolOutcome {
        &self.outcome
    }
}

pub(crate) fn validate_supported_results(
    supported: &BTreeSet<SupportedResultType>,
) -> Result<(), ToolRegistryError> {
    if supported.is_empty() {
        return Err(ToolRegistryError::InvalidDefinition(
            "tool must support at least one result type".to_owned(),
        ));
    }
    Ok(())
}

fn validate_opaque_id(value: &str) -> Result<(), ToolRegistryError> {
    if value.is_empty()
        || value.len() > MAX_REFERENCE_ID_BYTES
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        || value.contains('/')
        || value.contains('\\')
    {
        return Err(ToolRegistryError::InvalidResult(
            "opaque local reference is invalid".to_owned(),
        ));
    }
    Ok(())
}

fn is_lower_hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

impl fmt::Display for CancellationReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}
