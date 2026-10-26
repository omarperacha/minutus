use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{
    ExecutionLedgerError, ProjectCapability, ValidatedToolRequest,
    ledger_types::{validate_digest, validate_id, validate_message, validate_relative_path},
};

const MAX_RESOURCES: usize = 32;

/// Closed category of resource protected by a permission decision.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionResourceKind {
    /// One project workspace capability.
    Workspace,
    /// Project-scoped trusted application state.
    Project,
    /// One explicitly configured external service scope.
    External,
    /// One wallet or transaction authority scope.
    Wallet,
}

/// Exact project and capability resource scope for one request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PermissionResourceScope {
    project_id: String,
    kind: PermissionResourceKind,
    scope_id: String,
    scope_revision: String,
    resources: Vec<String>,
}

impl PermissionResourceScope {
    /// Creates a bounded trusted resource scope.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed identity, empty resources, or limits.
    pub fn new(
        project_id: impl Into<String>,
        kind: PermissionResourceKind,
        scope_id: impl Into<String>,
        scope_revision: impl Into<String>,
        resources: Vec<String>,
    ) -> Result<Self, ExecutionLedgerError> {
        let scope = Self {
            project_id: project_id.into(),
            kind,
            scope_id: scope_id.into(),
            scope_revision: scope_revision.into(),
            resources,
        };
        scope.validate()?;
        Ok(scope)
    }

    /// Returns the owning project.
    #[must_use]
    pub fn project_id(&self) -> &str {
        &self.project_id
    }
    /// Returns the resource category.
    #[must_use]
    pub const fn kind(&self) -> PermissionResourceKind {
        self.kind
    }
    /// Returns safe bounded resource labels such as relative paths.
    #[must_use]
    pub fn resources(&self) -> &[String] {
        &self.resources
    }

    pub(crate) fn validate(&self) -> Result<(), ExecutionLedgerError> {
        validate_id("permission project ID", &self.project_id)?;
        validate_id("permission scope ID", &self.scope_id)?;
        validate_id("permission scope revision", &self.scope_revision)?;
        if self.resources.is_empty() || self.resources.len() > MAX_RESOURCES {
            return Err(ExecutionLedgerError::LimitExceeded);
        }
        for resource in &self.resources {
            if self.kind == PermissionResourceKind::Workspace {
                validate_relative_path(resource)?;
            } else {
                validate_message("permission resource", resource)?;
            }
        }
        Ok(())
    }
}

/// Existing durable authority that may cover a capability-only request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DurableCapability {
    project_id: String,
    capability: ProjectCapability,
    scope_id: String,
    scope_revision: String,
}

impl DurableCapability {
    /// Creates a trusted durable capability resolved by the host.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed scope identity.
    pub fn new(
        project_id: impl Into<String>,
        capability: ProjectCapability,
        scope_id: impl Into<String>,
        scope_revision: impl Into<String>,
    ) -> Result<Self, ExecutionLedgerError> {
        if capability == ProjectCapability::NoProjectAuthority {
            return Err(ExecutionLedgerError::InvalidRecord(
                "no-authority tools cannot have durable capabilities".to_owned(),
            ));
        }
        let capability = Self {
            project_id: project_id.into(),
            capability,
            scope_id: scope_id.into(),
            scope_revision: scope_revision.into(),
        };
        validate_id("durable capability project ID", &capability.project_id)?;
        validate_id("durable capability scope ID", &capability.scope_id)?;
        validate_id("durable capability revision", &capability.scope_revision)?;
        Ok(capability)
    }

    pub(crate) fn covers(
        &self,
        required: ProjectCapability,
        scope: &PermissionResourceScope,
    ) -> bool {
        self.project_id == scope.project_id
            && self.capability == required
            && self.scope_id == scope.scope_id
            && self.scope_revision == scope.scope_revision
    }
}

/// Exact immutable permission identity for one validated request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PermissionBinding {
    call_id: String,
    tool_name: String,
    arguments_sha256: String,
    capability: ProjectCapability,
    resource_scope: PermissionResourceScope,
}

impl PermissionBinding {
    pub(crate) fn from_request(
        request: &ValidatedToolRequest,
        resource_scope: PermissionResourceScope,
    ) -> Result<Self, ExecutionLedgerError> {
        resource_scope.validate()?;
        Ok(Self {
            call_id: request.call_id().to_owned(),
            tool_name: request.tool_name().to_owned(),
            arguments_sha256: request_arguments_sha256(request.arguments()),
            capability: request.definition().capability(),
            resource_scope,
        })
    }

    /// Returns the exact call ID.
    #[must_use]
    pub fn call_id(&self) -> &str {
        &self.call_id
    }
    /// Returns the trusted registry tool name.
    #[must_use]
    pub fn tool_name(&self) -> &str {
        &self.tool_name
    }
    /// Returns the canonical argument digest.
    #[must_use]
    pub fn arguments_sha256(&self) -> &str {
        &self.arguments_sha256
    }
    /// Returns the required durable capability.
    #[must_use]
    pub const fn capability(&self) -> ProjectCapability {
        self.capability
    }
    /// Returns the exact trusted resource scope.
    #[must_use]
    pub const fn resource_scope(&self) -> &PermissionResourceScope {
        &self.resource_scope
    }

    pub(crate) fn validate(&self) -> Result<(), ExecutionLedgerError> {
        validate_id("permission call ID", &self.call_id)?;
        validate_id("permission tool name", &self.tool_name)?;
        validate_digest(&self.arguments_sha256)?;
        self.resource_scope.validate()
    }

    pub(crate) fn matches_request(
        &self,
        request: &ValidatedToolRequest,
        scope: &PermissionResourceScope,
    ) -> bool {
        self.matches_request_only(request) && &self.resource_scope == scope
    }

    pub(crate) fn matches_request_only(&self, request: &ValidatedToolRequest) -> bool {
        self.call_id == request.call_id()
            && self.tool_name == request.tool_name()
            && self.arguments_sha256 == request_arguments_sha256(request.arguments())
            && self.capability == request.definition().capability()
    }
}

/// Non-cloneable proof that trusted mediation authorized one exact request.
#[derive(Debug, Eq, PartialEq)]
pub struct ExecutionAuthorization {
    pub(crate) binding: PermissionBinding,
    pub(crate) source: AuthorizationSource,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AuthorizationSource {
    HostPolicy,
    DurableCapability,
    InteractiveApproval,
}

impl ExecutionAuthorization {
    /// Returns the exact authorized request binding.
    #[must_use]
    pub const fn binding(&self) -> &PermissionBinding {
        &self.binding
    }

    /// Returns whether this receipt covers the exact validated request and
    /// trusted resource scope.
    ///
    /// This check does not consume the non-cloneable receipt. A privileged
    /// composition adapter must still move the receipt into the one execution
    /// it authorizes.
    #[must_use]
    pub fn matches_request_and_scope(
        &self,
        request: &ValidatedToolRequest,
        scope: &PermissionResourceScope,
    ) -> bool {
        self.binding.matches_request(request, scope)
    }

    pub(crate) fn into_parts(self) -> (PermissionBinding, bool) {
        let interactive = matches!(self.source, AuthorizationSource::InteractiveApproval);
        (self.binding, interactive)
    }
}

pub(crate) fn request_arguments_sha256(arguments: &Value) -> String {
    let mut hasher = Sha256::new();
    hash_json(arguments, &mut hasher);
    format!("{:x}", hasher.finalize())
}

fn hash_json(value: &Value, hasher: &mut Sha256) {
    match value {
        Value::Null => hasher.update(b"n"),
        Value::Bool(value) => hasher.update(if *value { b"t" } else { b"f" }),
        Value::Number(value) => {
            hasher.update(b"d");
            hasher.update(value.to_string().as_bytes());
        }
        Value::String(value) => {
            hasher.update(b"s");
            hash_len(value.len(), hasher);
            hasher.update(value.as_bytes());
        }
        Value::Array(values) => {
            hasher.update(b"a");
            hash_len(values.len(), hasher);
            for value in values {
                hash_json(value, hasher);
            }
        }
        Value::Object(values) => {
            hasher.update(b"o");
            hash_len(values.len(), hasher);
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_unstable_by_key(|(left, _)| *left);
            for (key, value) in entries {
                hash_len(key.len(), hasher);
                hasher.update(key.as_bytes());
                hash_json(value, hasher);
            }
        }
    }
}

fn hash_len(length: usize, hasher: &mut Sha256) {
    hasher.update(length.to_le_bytes());
}
