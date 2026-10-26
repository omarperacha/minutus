use serde::{Deserialize, Serialize};

use super::{ExecutionLedgerError, validate_alias, validate_id, validate_relative_path};

const MAX_REQUEST_BYTES: usize = 64 * 1024;

/// Immutable identity and text for the user request owning one ledger.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LedgerRequest {
    request_id: String,
    conversation_id: String,
    project_id: String,
    text: String,
}

impl LedgerRequest {
    /// Creates a bounded request scope.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed IDs or empty/oversized request text.
    pub fn new(
        request_id: impl Into<String>,
        conversation_id: impl Into<String>,
        project_id: impl Into<String>,
        text: impl Into<String>,
    ) -> Result<Self, ExecutionLedgerError> {
        let request = Self {
            request_id: request_id.into(),
            conversation_id: conversation_id.into(),
            project_id: project_id.into(),
            text: text.into(),
        };
        request.validate()?;
        Ok(request)
    }

    /// Returns the request ID.
    #[must_use]
    pub fn request_id(&self) -> &str {
        &self.request_id
    }
    /// Returns the owning conversation ID.
    #[must_use]
    pub fn conversation_id(&self) -> &str {
        &self.conversation_id
    }
    /// Returns the project security scope.
    #[must_use]
    pub fn project_id(&self) -> &str {
        &self.project_id
    }
    /// Returns the current user request text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    pub(crate) fn validate(&self) -> Result<(), ExecutionLedgerError> {
        validate_id("request ID", &self.request_id)?;
        validate_id("conversation ID", &self.conversation_id)?;
        validate_id("project ID", &self.project_id)?;
        if self.text.is_empty() || self.text.len() > MAX_REQUEST_BYTES {
            return Err(ExecutionLedgerError::InvalidRecord(
                "current request text is invalid".to_owned(),
            ));
        }
        Ok(())
    }
}

/// Capability level retained only in trusted host state.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LedgerCapability {
    /// The workspace may only be observed.
    Read,
    /// The workspace may be read and mutated.
    ReadWrite,
}

/// Host-only resolution of one model-facing workspace alias.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceResolution {
    alias: String,
    capability_id: String,
    capability_revision: String,
    capability: LedgerCapability,
}

impl WorkspaceResolution {
    /// Creates a capability-bound alias without storing a private root.
    ///
    /// # Errors
    /// Returns an error for malformed aliases or opaque capability identifiers.
    pub fn new(
        alias: impl Into<String>,
        capability_id: impl Into<String>,
        capability_revision: impl Into<String>,
        capability: LedgerCapability,
    ) -> Result<Self, ExecutionLedgerError> {
        let resolution = Self {
            alias: alias.into(),
            capability_id: capability_id.into(),
            capability_revision: capability_revision.into(),
            capability,
        };
        resolution.validate()?;
        Ok(resolution)
    }
    /// Returns the non-authority-bearing model alias.
    #[must_use]
    pub fn alias(&self) -> &str {
        &self.alias
    }
    pub(crate) fn capability_id(&self) -> &str {
        &self.capability_id
    }
    pub(crate) fn capability_revision(&self) -> &str {
        &self.capability_revision
    }
    pub(crate) const fn capability(&self) -> LedgerCapability {
        self.capability
    }
    pub(crate) fn validate(&self) -> Result<(), ExecutionLedgerError> {
        validate_alias(&self.alias)?;
        validate_id("capability ID", &self.capability_id)?;
        validate_id("capability revision", &self.capability_revision)
    }
}

/// Exact request/result/project provenance for one ledger fact.
#[allow(clippy::struct_field_names)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FactOrigin {
    request_id: String,
    project_id: String,
    tool_call_id: String,
    result_id: Option<String>,
}

impl FactOrigin {
    pub(crate) fn new(
        request_id: &str,
        project_id: &str,
        tool_call_id: &str,
        result_id: Option<&str>,
    ) -> Result<Self, ExecutionLedgerError> {
        let origin = Self {
            request_id: request_id.to_owned(),
            project_id: project_id.to_owned(),
            tool_call_id: tool_call_id.to_owned(),
            result_id: result_id.map(str::to_owned),
        };
        origin.validate()?;
        Ok(origin)
    }
    /// Returns the originating user request ID.
    #[must_use]
    pub fn request_id(&self) -> &str {
        &self.request_id
    }
    /// Returns the originating project scope.
    #[must_use]
    pub fn project_id(&self) -> &str {
        &self.project_id
    }
    /// Returns the exact tool call ID.
    #[must_use]
    pub fn tool_call_id(&self) -> &str {
        &self.tool_call_id
    }
    /// Returns the host-generated terminal result ID when one exists.
    #[must_use]
    pub fn result_id(&self) -> Option<&str> {
        self.result_id.as_deref()
    }
    pub(crate) fn validate(&self) -> Result<(), ExecutionLedgerError> {
        validate_id("origin request ID", &self.request_id)?;
        validate_id("origin project ID", &self.project_id)?;
        validate_id("origin tool call ID", &self.tool_call_id)?;
        if let Some(result_id) = &self.result_id {
            validate_id("origin result ID", result_id)?;
        }
        Ok(())
    }
}

/// Capability-scoped workspace-relative resource identity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LedgerResource {
    workspace_alias: String,
    relative_path: String,
    capability_id: String,
}

impl LedgerResource {
    pub(crate) fn new(
        workspace: &WorkspaceResolution,
        relative_path: impl Into<String>,
    ) -> Result<Self, ExecutionLedgerError> {
        let resource = Self {
            workspace_alias: workspace.alias.clone(),
            relative_path: relative_path.into(),
            capability_id: workspace.capability_id.clone(),
        };
        resource.validate()?;
        Ok(resource)
    }
    /// Returns the model-facing workspace alias.
    #[must_use]
    pub fn workspace_alias(&self) -> &str {
        &self.workspace_alias
    }
    /// Returns the validated workspace-relative path.
    #[must_use]
    pub fn relative_path(&self) -> &str {
        &self.relative_path
    }
    pub(crate) fn capability_id(&self) -> &str {
        &self.capability_id
    }
    pub(crate) fn validate(&self) -> Result<(), ExecutionLedgerError> {
        validate_alias(&self.workspace_alias)?;
        validate_relative_path(&self.relative_path)?;
        validate_id("resource capability ID", &self.capability_id)
    }
}

/// Exact assumptions required before reusing an earlier observation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReuseContext {
    /// Current project ID.
    pub project_id: String,
    /// Current workspace alias.
    pub workspace_alias: String,
    /// Current durable capability ID.
    pub capability_id: String,
    /// Current capability revision.
    pub capability_revision: String,
    /// Current resource revision.
    pub resource_revision: String,
}
