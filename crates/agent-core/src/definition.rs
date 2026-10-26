use std::{collections::BTreeSet, time::Duration};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    error::ToolRegistryError,
    result::{SupportedResultType, validate_supported_results},
    schema::StrictJsonSchema,
};

const MAX_TOOL_NAME_BYTES: usize = 128;
const MAX_DESCRIPTION_BYTES: usize = 4_096;
const MAX_TIMEOUT_MS: u64 = 300_000;
const MIN_OUTPUT_BYTES: usize = 512;
const MAX_OUTPUT_BYTES: usize = 1_048_576;
const MAX_RESULT_PARTS: usize = 64;

/// Trusted effect classification for a tool.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ToolClassification {
    /// Observes existing local state.
    Read,
    /// Writes user-controlled project data.
    Write,
    /// Mutates trusted application state without directly writing project data.
    Mutation,
    /// Can affect an external system or counterparty.
    ExternalSideEffect,
}

/// Durable project capability required before execution.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProjectCapability {
    /// No project resource authority is required or granted.
    NoProjectAuthority,
    /// Read from one workspace attached to the active project.
    WorkspaceRead,
    /// Write inside one read-and-write workspace attached to the active project.
    WorkspaceWrite,
    /// Use explicitly permitted external networking.
    ExternalNetwork,
    /// Construct a transaction proposal without signing it.
    TransactionProposal,
    /// Ask the wallet broker to sign or submit an exact approved action.
    WalletSigning,
}

/// Whether a durable capability is sufficient or one exact call needs approval.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalRequirement {
    /// The existing project capability is sufficient.
    CapabilityOnly,
    /// Trusted UI approval is required for this exact validated request.
    Interactive,
}

/// Coarse trusted risk indicator for policy and presentation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RiskLevel {
    /// Bounded observation with negligible impact.
    Low,
    /// Reversible or contained mutation.
    Medium,
    /// Sensitive, difficult-to-reverse, or externally visible effect.
    High,
    /// Signing, funds, or similarly critical authority.
    Critical,
}

/// Per-call execution and result bounds enforced by trusted runtime state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolLimits {
    /// Wall-clock execution deadline in milliseconds.
    pub timeout_ms: u64,
    /// Maximum serialized result-envelope size.
    pub max_output_bytes: usize,
    /// Maximum number of completed result content parts.
    pub max_result_parts: usize,
}

impl ToolLimits {
    /// Creates validated tool bounds.
    ///
    /// # Errors
    ///
    /// Returns an error when a limit is zero or exceeds its host safety ceiling.
    pub fn new(
        timeout: Duration,
        max_output_bytes: usize,
        max_result_parts: usize,
    ) -> Result<Self, ToolRegistryError> {
        let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
        if timeout_ms == 0 || timeout_ms > MAX_TIMEOUT_MS {
            return Err(ToolRegistryError::InvalidDefinition(
                "tool timeout is outside the allowed range".to_owned(),
            ));
        }
        if !(MIN_OUTPUT_BYTES..=MAX_OUTPUT_BYTES).contains(&max_output_bytes) {
            return Err(ToolRegistryError::InvalidDefinition(
                "tool output limit is outside the allowed range".to_owned(),
            ));
        }
        if max_result_parts == 0 || max_result_parts > MAX_RESULT_PARTS {
            return Err(ToolRegistryError::InvalidDefinition(
                "tool result-part limit is outside the allowed range".to_owned(),
            ));
        }
        Ok(Self {
            timeout_ms,
            max_output_bytes,
            max_result_parts,
        })
    }
}

/// Complete trusted metadata for one tool registry entry.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolDefinition {
    name: String,
    description: String,
    input_schema: StrictJsonSchema,
    classification: ToolClassification,
    capability: ProjectCapability,
    approval: ApprovalRequirement,
    risk: RiskLevel,
    limits: ToolLimits,
    supported_results: BTreeSet<SupportedResultType>,
}

impl ToolDefinition {
    /// Builds and validates a trusted tool definition.
    ///
    /// # Errors
    ///
    /// Returns an error when metadata, schema, limits, or result types violate
    /// the registry contract.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: Value,
        classification: ToolClassification,
        capability: ProjectCapability,
        approval: ApprovalRequirement,
        risk: RiskLevel,
        limits: ToolLimits,
        supported_results: impl IntoIterator<Item = SupportedResultType>,
    ) -> Result<Self, ToolRegistryError> {
        let name = name.into();
        let description = description.into();
        validate_tool_name(&name)?;
        if description.is_empty()
            || description.len() > MAX_DESCRIPTION_BYTES
            || description.chars().any(char::is_control)
        {
            return Err(ToolRegistryError::InvalidDefinition(
                "tool description is invalid".to_owned(),
            ));
        }
        let input_schema = StrictJsonSchema::compile(input_schema).map_err(|error| {
            ToolRegistryError::InvalidSchema {
                path: error.path().to_owned(),
                message: error.message().to_owned(),
            }
        })?;
        let supported_results = supported_results.into_iter().collect::<BTreeSet<_>>();
        validate_supported_results(&supported_results)?;
        Ok(Self {
            name,
            description,
            input_schema,
            classification,
            capability,
            approval,
            risk,
            limits,
            supported_results,
        })
    }

    /// Returns the stable namespaced tool name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the model-facing description supplied by trusted state.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Returns the strict JSON argument schema.
    #[must_use]
    pub fn input_schema(&self) -> &Value {
        self.input_schema.as_value()
    }

    /// Returns the effect classification.
    #[must_use]
    pub const fn classification(&self) -> ToolClassification {
        self.classification
    }

    /// Returns the required durable project capability.
    #[must_use]
    pub const fn capability(&self) -> ProjectCapability {
        self.capability
    }

    /// Returns the approval policy.
    #[must_use]
    pub const fn approval(&self) -> ApprovalRequirement {
        self.approval
    }

    /// Returns the trusted risk level.
    #[must_use]
    pub const fn risk(&self) -> RiskLevel {
        self.risk
    }

    /// Returns per-call execution bounds.
    #[must_use]
    pub const fn limits(&self) -> ToolLimits {
        self.limits
    }

    /// Returns the allow-listed result types.
    #[must_use]
    pub const fn supported_results(&self) -> &BTreeSet<SupportedResultType> {
        &self.supported_results
    }

    pub(crate) fn validate_arguments(&self, arguments: &Value) -> Result<(), ToolRegistryError> {
        self.input_schema
            .validate(arguments)
            .map_err(|error| ToolRegistryError::InvalidArguments {
                path: error.path().to_owned(),
                message: error.message().to_owned(),
            })
    }

    pub(crate) fn model_projection(&self) -> ModelToolDefinition {
        ModelToolDefinition {
            name: self.name.clone(),
            description: self.description.clone(),
            parameters: self.input_schema.as_value().clone(),
        }
    }
}

/// Model-neutral projection consumed by the active adapter.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ModelToolDefinition {
    /// Stable namespaced recipient/function name.
    pub name: String,
    /// Trusted model-facing guidance.
    pub description: String,
    /// Strict JSON Schema for arguments.
    pub parameters: Value,
}

fn validate_tool_name(name: &str) -> Result<(), ToolRegistryError> {
    if name.is_empty() || name.len() > MAX_TOOL_NAME_BYTES {
        return Err(ToolRegistryError::InvalidDefinition(
            "tool name is invalid".to_owned(),
        ));
    }
    for segment in name.split('.') {
        let mut bytes = segment.bytes();
        if bytes.next().is_none_or(|byte| !byte.is_ascii_lowercase())
            || bytes
                .any(|byte| !(byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'))
        {
            return Err(ToolRegistryError::InvalidDefinition(
                "tool name is invalid".to_owned(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn definition(schema: Value) -> Result<ToolDefinition, ToolRegistryError> {
        ToolDefinition::new(
            "filesystem.read_text",
            "Read text.",
            schema,
            ToolClassification::Read,
            ProjectCapability::WorkspaceRead,
            ApprovalRequirement::CapabilityOnly,
            RiskLevel::Low,
            ToolLimits::new(Duration::from_secs(5), 2_048, 3)?,
            [SupportedResultType::Text],
        )
    }

    #[test]
    fn malformed_and_open_schemas_fail_closed() {
        assert!(
            definition(serde_json::json!({"type":"object","properties":{},"required":[]})).is_err()
        );
        assert!(definition(serde_json::json!({"type":"object","properties":{},"required":[],"additionalProperties":false,"$ref":"evil"})).is_err());
        assert!(definition(serde_json::json!({"type":"object","properties":{"nested":{"type":"object","properties":{},"required":[]}},"required":["nested"],"additionalProperties":false})).is_err());
    }

    #[test]
    fn execution_limits_enforce_boundaries() {
        assert!(ToolLimits::new(Duration::ZERO, MIN_OUTPUT_BYTES, 1).is_err());
        assert!(ToolLimits::new(Duration::from_secs(1), MIN_OUTPUT_BYTES - 1, 1).is_err());
        assert!(ToolLimits::new(Duration::from_secs(1), MAX_OUTPUT_BYTES + 1, 1).is_err());
        assert!(
            ToolLimits::new(
                Duration::from_secs(1),
                MIN_OUTPUT_BYTES,
                MAX_RESULT_PARTS + 1
            )
            .is_err()
        );
    }
}
