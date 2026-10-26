use std::collections::BTreeSet;

use serde_json::Value;

use crate::{
    definition::{ToolDefinition, ToolLimits},
    error::ToolRegistryError,
    result::SupportedResultType,
};

const MAX_CALL_ID_BYTES: usize = 160;

/// Raw model-adapter output. This type carries no authority.
#[derive(Clone, Debug, PartialEq)]
pub struct UntrustedToolRequest {
    /// Adapter-scoped invocation ID.
    pub call_id: String,
    /// Model-selected tool name.
    pub name: String,
    /// Model-selected arguments.
    pub arguments: Value,
}

impl UntrustedToolRequest {
    pub(crate) fn validate_call_id(&self) -> Result<(), ToolRegistryError> {
        if self.call_id.is_empty()
            || self.call_id.len() > MAX_CALL_ID_BYTES
            || self
                .call_id
                .chars()
                .any(|character| character.is_control() || character.is_whitespace())
        {
            return Err(ToolRegistryError::InvalidCallId);
        }
        Ok(())
    }
}

/// A request proven against one immutable registry entry.
#[derive(Clone, Debug, PartialEq)]
pub struct ValidatedToolRequest {
    call_id: String,
    definition: ToolDefinition,
    arguments: Value,
}

impl ValidatedToolRequest {
    pub(crate) fn new(call_id: String, definition: ToolDefinition, arguments: Value) -> Self {
        Self {
            call_id,
            definition,
            arguments,
        }
    }

    /// Returns the invocation ID.
    #[must_use]
    pub fn call_id(&self) -> &str {
        &self.call_id
    }

    /// Returns the trusted tool name, never a separate model value.
    #[must_use]
    pub fn tool_name(&self) -> &str {
        self.definition.name()
    }

    /// Returns schema-validated arguments.
    #[must_use]
    pub const fn arguments(&self) -> &Value {
        &self.arguments
    }

    /// Returns the trusted metadata bound to this request.
    #[must_use]
    pub const fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    pub(crate) const fn limits(&self) -> ToolLimits {
        self.definition.limits()
    }

    pub(crate) const fn supported_results(&self) -> &BTreeSet<SupportedResultType> {
        self.definition.supported_results()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_call_ids_fail_before_registry_lookup() {
        for call_id in ["", "with whitespace", "line\nbreak"] {
            let request = UntrustedToolRequest {
                call_id: call_id.to_owned(),
                name: "filesystem.read_text".to_owned(),
                arguments: serde_json::json!({}),
            };
            assert_eq!(
                request.validate_call_id(),
                Err(ToolRegistryError::InvalidCallId)
            );
        }
    }
}
