//! Closed model-visible advisory execution-forecast schema.

use model_runtime::PromptToolDefinition;

use super::MAX_PROVISIONAL_PLAN_STEPS;

pub(crate) const LEGACY_EXECUTION_FORECAST_CONTRACT_VERSION: &str = "execution_forecast_v1";
pub(crate) const ROLE_AWARE_EXECUTION_FORECAST_CONTRACT_VERSION: &str = "execution_forecast_v2";
pub(crate) const EXECUTION_FORECAST_CONTRACT_VERSION: &str = "execution_forecast_v3";

fn route_schema(definition: &PromptToolDefinition) -> serde_json::Value {
    let arguments = definition.parameters["properties"]
        .as_object()
        .map(|properties| {
            properties
                .keys()
                .map(|argument| serde_json::Value::String(argument.clone()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let required = definition.parameters["required"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let argument_list = |description: String| {
        if arguments.is_empty() {
            serde_json::json!({
                "type": "array", "minItems": 0, "maxItems": 0, "uniqueItems": true,
                "description": description
            })
        } else {
            serde_json::json!({
                "type": "array", "minItems": 0, "maxItems": 16, "uniqueItems": true,
                "description": description,
                "items": { "type": "string", "enum": arguments }
            })
        }
    };
    let known_arguments = argument_list(format!(
        "Arguments already resolved. Required arguments for this operation: {}.",
        serde_json::Value::Array(required.clone())
    ));
    let unresolved_arguments = argument_list(format!(
        "Arguments still requiring evidence. Every required argument must appear in exactly one argument list. Required arguments: {}.",
        serde_json::Value::Array(required)
    ));
    serde_json::json!({
        "type": "object",
        "properties": {
            "operation": { "type": "string", "const": definition.name },
            "knownArguments": known_arguments,
            "unresolvedArguments": unresolved_arguments
        },
        "required": ["operation", "knownArguments", "unresolvedArguments"],
        "additionalProperties": false
    })
}

pub(super) fn tool_definition(
    forecastable_tools: &[PromptToolDefinition],
    forecastable_categories: &[String],
) -> PromptToolDefinition {
    let route_schemas = forecastable_tools
        .iter()
        .map(route_schema)
        .collect::<Vec<_>>();
    PromptToolDefinition {
        name: "orchestration.submit_forecast".to_owned(),
        description: format!(
            "After semantic_execution_v4 plan acceptance, submit one concise advisory forecast before task operations. Cover every semantic step exactly once with a catalogue category, a small operation route, dependencies, the exact obligation IDs, a compatible postcondition, and known versus unresolved argument names. Forecast only tool-backed work. Suggestions grant no permission or progress. Up to {MAX_PROVISIONAL_PLAN_STEPS} entries."
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "contractVersion": { "type": "string", "enum": [EXECUTION_FORECAST_CONTRACT_VERSION] },
                "mode": { "type": "string", "enum": ["initial", "revise"] },
                "entries": {
                    "type": "array", "minItems": 1, "maxItems": MAX_PROVISIONAL_PLAN_STEPS,
                    "items": {
                        "type": "object",
                        "properties": {
                            "stepId": { "type": "string", "minLength": 1, "maxLength": 32 },
                            "category": { "type": "string", "enum": forecastable_categories },
                            "purpose": { "type": "string", "minLength": 1, "maxLength": 256 },
                            "dependsOn": { "type": "array", "minItems": 0, "maxItems": 15, "uniqueItems": true, "items": { "type": "string", "minLength": 1, "maxLength": 32 } },
                            "requiredObligations": { "type": "array", "minItems": 1, "maxItems": 8, "uniqueItems": true, "items": { "type": "string", "minLength": 1, "maxLength": 32, "pattern": "^[a-z][a-z0-9-]*$" } },
                            "postcondition": { "type": "string", "enum": ["evidence_ready", "content_loaded", "computation_completed", "file_write_verified", "directory_create_verified", "external_effect_verified"] },
                            "routes": {
                                "type": "array", "minItems": 1, "maxItems": 3,
                                "items": { "oneOf": route_schemas }
                            }
                        },
                        "required": ["stepId", "category", "purpose", "dependsOn", "requiredObligations", "postcondition", "routes"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["contractVersion", "mode", "entries"],
            "additionalProperties": false
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v3_forecast_schema_covers_exact_v4_obligation_ids() -> Result<(), ipc_types::IpcError> {
        let definition = tool_definition(
            &[PromptToolDefinition {
                name: "filesystem.read_text".to_owned(),
                description: String::new(),
                parameters: serde_json::json!({
                    "type":"object",
                    "properties":{"workspace":{"type":"string"},"path":{"type":"string"}},
                    "required":["workspace","path"],
                    "additionalProperties":false
                }),
            }],
            &["workspace_read".to_owned()],
        );
        let entry = &definition.parameters["properties"]["entries"]["items"];
        let required = entry["required"]
            .as_array()
            .ok_or_else(ipc_types::IpcError::internal)?;
        assert_eq!(
            definition.parameters["properties"]["contractVersion"]["enum"][0],
            EXECUTION_FORECAST_CONTRACT_VERSION
        );
        assert!(required.iter().any(|field| field == "requiredObligations"));
        assert!(!required.iter().any(|field| field == "requiredEvidence"));
        let route = &entry["properties"]["routes"]["items"]["oneOf"][0];
        assert_eq!(
            route["properties"]["operation"]["const"],
            "filesystem.read_text"
        );
        assert_eq!(
            route["properties"]["knownArguments"]["items"]["enum"],
            serde_json::json!(["path", "workspace"])
        );
        Ok(())
    }
}
