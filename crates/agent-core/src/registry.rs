use std::collections::BTreeMap;

use crate::{
    definition::{ModelToolDefinition, ToolDefinition},
    error::ToolRegistryError,
    request::{UntrustedToolRequest, ValidatedToolRequest},
    result::{CancellationReason, ResultContent, ToolFailure, ToolResultEnvelope},
};

const MAX_TOOLS: usize = 128;

/// Immutable lookup table for all tools available in one runtime context.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolRegistry {
    definitions: BTreeMap<String, ToolDefinition>,
}

impl ToolRegistry {
    /// Creates a registry from trusted Rust-owned definitions.
    ///
    /// # Errors
    ///
    /// Returns an error for duplicate definitions or too many entries.
    pub fn new(
        definitions: impl IntoIterator<Item = ToolDefinition>,
    ) -> Result<Self, ToolRegistryError> {
        let mut entries = BTreeMap::new();
        for definition in definitions {
            let name = definition.name().to_owned();
            if entries.insert(name.clone(), definition).is_some() {
                return Err(ToolRegistryError::DuplicateTool(name));
            }
            if entries.len() > MAX_TOOLS {
                return Err(ToolRegistryError::InvalidDefinition(
                    "tool registry exceeds the entry limit".to_owned(),
                ));
            }
        }
        Ok(Self {
            definitions: entries,
        })
    }

    /// Returns trusted entries in deterministic name order.
    #[must_use]
    pub fn definitions(&self) -> impl ExactSizeIterator<Item = &ToolDefinition> {
        self.definitions.values()
    }

    /// Projects registry entries without exposing capability or policy authority to the model.
    #[must_use]
    pub fn model_definitions(&self) -> Vec<ModelToolDefinition> {
        self.definitions
            .values()
            .map(ToolDefinition::model_projection)
            .collect()
    }

    /// Rejects unknown tools and malformed or schema-invalid arguments before execution.
    ///
    /// # Errors
    ///
    /// Returns an error when the call ID, name, or arguments do not exactly
    /// match the active trusted registry.
    pub fn validate_request(
        &self,
        request: UntrustedToolRequest,
    ) -> Result<ValidatedToolRequest, ToolRegistryError> {
        request.validate_call_id()?;
        let definition = self
            .definitions
            .get(&request.name)
            .ok_or_else(|| ToolRegistryError::UnknownTool(request.name.clone()))?;
        definition.validate_arguments(&request.arguments)?;
        Ok(ValidatedToolRequest::new(
            request.call_id,
            definition.clone(),
            request.arguments,
        ))
    }

    /// Creates a bounded successful result for an exact validated request.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale request, unsupported result type, or an
    /// oversized envelope.
    pub fn completed(
        &self,
        request: &ValidatedToolRequest,
        content: Vec<ResultContent>,
    ) -> Result<ToolResultEnvelope, ToolRegistryError> {
        self.ensure_current(request)?;
        ToolResultEnvelope::completed(request, content)
    }

    /// Creates a bounded failed result for an exact validated request.
    ///
    /// # Errors
    ///
    /// Returns an error when the request is stale or the envelope is oversized.
    pub fn failed(
        &self,
        request: &ValidatedToolRequest,
        failure: ToolFailure,
    ) -> Result<ToolResultEnvelope, ToolRegistryError> {
        self.ensure_current(request)?;
        ToolResultEnvelope::failed(request, failure)
    }

    /// Creates a bounded cancelled result for an exact validated request.
    ///
    /// # Errors
    ///
    /// Returns an error when the request is stale or the envelope is oversized.
    pub fn cancelled(
        &self,
        request: &ValidatedToolRequest,
        reason: CancellationReason,
    ) -> Result<ToolResultEnvelope, ToolRegistryError> {
        self.ensure_current(request)?;
        ToolResultEnvelope::cancelled(request, reason)
    }

    fn ensure_current(&self, request: &ValidatedToolRequest) -> Result<(), ToolRegistryError> {
        if self.definitions.get(request.tool_name()) != Some(request.definition()) {
            return Err(ToolRegistryError::StaleValidatedRequest);
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
