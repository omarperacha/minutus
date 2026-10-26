use std::{error::Error, fmt, path::Path};

use serde::{Deserialize, Serialize};

const MAX_TOOL_NAME_BYTES: usize = 256;
const MAX_REFERENCE_BYTES: usize = 160;
const MAX_ALIAS_BYTES: usize = 80;
const MAX_RELATIVE_PATH_BYTES: usize = 512;

/// Host-owned category describing where untrusted prompt content originated.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UntrustedSourceKind {
    /// Content read from a project workspace.
    WorkspaceFile,
    /// General content returned by a registered tool.
    ToolResult,
    /// Content returned by a remote x402-capable service.
    RemoteResponse,
    /// Output produced by a future sandboxed computation broker.
    SandboxOutput,
    /// Source or bound data supplied to a sandboxed computation broker.
    SandboxInput,
}

/// A bounded, non-authority-bearing resource reference safe for an indicator.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum BoundedResourceReference {
    /// A relative path under an opaque workspace alias.
    Workspace {
        /// User-visible workspace alias, never a bookmark or absolute root.
        alias: String,
        /// Normalized relative path within that workspace.
        relative_path: String,
    },
    /// An opaque broker-owned result, response or output identifier.
    Opaque {
        /// Identifier with no path, URL, credential or capability semantics.
        id: String,
    },
}

impl BoundedResourceReference {
    fn workspace(alias: String, relative_path: String) -> Result<Self, PromptSafetyError> {
        validate_opaque_component(&alias, MAX_ALIAS_BYTES)?;
        if relative_path.is_empty()
            || relative_path.len() > MAX_RELATIVE_PATH_BYTES
            || relative_path.contains('\\')
            || relative_path.chars().any(char::is_control)
            || Path::new(&relative_path).is_absolute()
            || !Path::new(&relative_path)
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_)))
        {
            return Err(PromptSafetyError::InvalidSourceReference);
        }
        Ok(Self::Workspace {
            alias,
            relative_path,
        })
    }

    fn opaque(id: String) -> Result<Self, PromptSafetyError> {
        validate_opaque_component(&id, MAX_REFERENCE_BYTES)?;
        Ok(Self::Opaque { id })
    }
}

/// Trusted provenance attached to an untrusted content boundary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UntrustedContentSource {
    kind: UntrustedSourceKind,
    source_tool: String,
    resource: BoundedResourceReference,
}

impl UntrustedContentSource {
    /// Creates provenance for a workspace file without accepting an absolute path.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid tool, alias, absolute path or escaping path.
    pub fn workspace_file(
        source_tool: impl Into<String>,
        workspace_alias: impl Into<String>,
        relative_path: impl Into<String>,
    ) -> Result<Self, PromptSafetyError> {
        Self::new(
            UntrustedSourceKind::WorkspaceFile,
            source_tool.into(),
            BoundedResourceReference::workspace(workspace_alias.into(), relative_path.into())?,
        )
    }

    /// Creates provenance for a general tool result.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid tool name or non-opaque result ID.
    pub fn tool_result(
        source_tool: impl Into<String>,
        result_id: impl Into<String>,
    ) -> Result<Self, PromptSafetyError> {
        Self::with_opaque_reference(UntrustedSourceKind::ToolResult, source_tool, result_id)
    }

    /// Creates provenance for a bounded remote response retained by the host.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid tool name or path/URL-like response ID.
    pub fn remote_response(
        source_tool: impl Into<String>,
        response_id: impl Into<String>,
    ) -> Result<Self, PromptSafetyError> {
        Self::with_opaque_reference(
            UntrustedSourceKind::RemoteResponse,
            source_tool,
            response_id,
        )
    }

    /// Creates provenance for output retained by a sandbox broker.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid tool name or non-opaque output ID.
    pub fn sandbox_output(
        source_tool: impl Into<String>,
        output_id: impl Into<String>,
    ) -> Result<Self, PromptSafetyError> {
        Self::with_opaque_reference(UntrustedSourceKind::SandboxOutput, source_tool, output_id)
    }

    /// Creates provenance for source or data supplied to a sandbox broker.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid tool name or non-opaque input ID.
    pub fn sandbox_input(
        source_tool: impl Into<String>,
        input_id: impl Into<String>,
    ) -> Result<Self, PromptSafetyError> {
        Self::with_opaque_reference(UntrustedSourceKind::SandboxInput, source_tool, input_id)
    }

    fn with_opaque_reference(
        kind: UntrustedSourceKind,
        source_tool: impl Into<String>,
        id: impl Into<String>,
    ) -> Result<Self, PromptSafetyError> {
        Self::new(
            kind,
            source_tool.into(),
            BoundedResourceReference::opaque(id.into())?,
        )
    }

    fn new(
        kind: UntrustedSourceKind,
        source_tool: String,
        resource: BoundedResourceReference,
    ) -> Result<Self, PromptSafetyError> {
        if source_tool.is_empty()
            || source_tool.len() > MAX_TOOL_NAME_BYTES
            || !source_tool.split('.').all(valid_identifier_part)
        {
            return Err(PromptSafetyError::InvalidSourceTool);
        }
        Ok(Self {
            kind,
            source_tool,
            resource,
        })
    }

    /// Returns the host-owned source category.
    #[must_use]
    pub const fn kind(&self) -> UntrustedSourceKind {
        self.kind
    }

    /// Returns the trusted registry tool name that produced the content.
    #[must_use]
    pub fn source_tool(&self) -> &str {
        &self.source_tool
    }

    /// Returns the bounded, non-authority-bearing resource reference.
    #[must_use]
    pub const fn resource(&self) -> &BoundedResourceReference {
        &self.resource
    }
}

/// Host-detected suspicious instruction categories.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum InjectionSignal {
    /// Content attempts to override or reorder trusted instructions.
    InstructionOverride,
    /// Content claims permission, approval or system authority.
    AuthorityEscalation,
    /// Content asks for credentials or other sensitive material to be disclosed.
    SensitiveDataExfiltration,
    /// A suspicious instruction was found after bounded host-side decoding.
    EncodedInstruction,
}

/// Authority-free UI indicator generated only by trusted host inspection.
///
/// The detected content and model reasoning are deliberately absent. This type
/// carries no capability handle, approval receipt or ledger mutation method.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptInjectionIndicator {
    source: UntrustedContentSource,
    signals: Vec<InjectionSignal>,
}

impl PromptInjectionIndicator {
    pub(crate) fn new(source: UntrustedContentSource, signals: Vec<InjectionSignal>) -> Self {
        Self { source, signals }
    }

    /// Returns safe provenance for separate trusted-host presentation.
    #[must_use]
    pub const fn source(&self) -> &UntrustedContentSource {
        &self.source
    }

    /// Returns deduplicated, deterministic signal categories.
    #[must_use]
    pub fn signals(&self) -> &[InjectionSignal] {
        &self.signals
    }
}

/// Fail-closed prompt-safety input or provenance error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PromptSafetyError {
    /// Registry tool name is not a bounded namespaced identifier.
    InvalidSourceTool,
    /// Resource reference could disclose or escape its bounded scope.
    InvalidSourceReference,
    /// Text or structured input exceeds host inspection limits.
    ContentTooLarge,
    /// Structured content exceeds the bounded traversal depth.
    ContentTooDeep,
}

impl fmt::Display for PromptSafetyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidSourceTool => "invalid prompt-safety source tool",
            Self::InvalidSourceReference => "invalid prompt-safety source reference",
            Self::ContentTooLarge => "untrusted content exceeds prompt-safety limits",
            Self::ContentTooDeep => "untrusted content nesting exceeds prompt-safety limits",
        };
        formatter.write_str(message)
    }
}

impl Error for PromptSafetyError {}

fn validate_opaque_component(value: &str, maximum: usize) -> Result<(), PromptSafetyError> {
    if value.is_empty()
        || value.len() > maximum
        || value.chars().any(|character| {
            character.is_control() || character.is_whitespace() || matches!(character, '/' | '\\')
        })
        || value.contains("://")
    {
        return Err(PromptSafetyError::InvalidSourceReference);
    }
    Ok(())
}

fn valid_identifier_part(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}
