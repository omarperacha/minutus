//! AGENT-001 registrations for pure and project-data sandboxed computation.

use agent_core::{
    ApprovalRequirement, ProjectCapability, RiskLevel, SupportedResultType, ToolClassification,
    ToolDefinition, ToolLimits, ToolRegistry, ToolRegistryError,
};
use computation_runtime::{MAX_INPUTS, MAX_OUTPUT_BYTES, MAX_SOURCE_BYTES, WALL_TIME_LIMIT};

/// Registry name for computation that receives no project authority.
pub const PURE_COMPUTATION_TOOL: &str = "computation.run";
/// Registry name for computation over explicitly bound project files.
pub const PROJECT_COMPUTATION_TOOL: &str = "computation.run_project";

/// Builds the immutable trusted computation subset of the AGENT-001 registry.
///
/// # Errors
///
/// Returns an error if a trusted schema or metadata constant violates the
/// generic registry contract.
pub fn computation_tool_registry() -> Result<ToolRegistry, ToolRegistryError> {
    ToolRegistry::new([
        ToolDefinition::new(
            PURE_COMPUTATION_TOOL,
            "Run one deterministic, synchronous JavaScript computation without project files. `source` must be one ECMAScript module with exactly one default export: `export default function compute(zeta) { ... }`. The function must return the declared text or JSON value directly; it cannot use inputs, imports, filesystem, network, environment, clocks, randomness, processes or packages. After success, answer the user normally; never run another computation merely to format or explain its result. Prefer a direct answer when no calculation or transformation is needed. Complete argument example: {\"source\":\"export default function compute(zeta) { return [4, 6].length; }\",\"output\":{\"kind\":\"json\",\"maxBytes\":128,\"maxItems\":1}}.",
            computation_schema(false),
            ToolClassification::Read,
            ProjectCapability::NoProjectAuthority,
            ApprovalRequirement::CapabilityOnly,
            RiskLevel::Low,
            ToolLimits::new(WALL_TIME_LIMIT, 64 * 1024, 1)?,
            [
                SupportedResultType::Text,
                SupportedResultType::Json,
                SupportedResultType::OpaqueLocalReference,
            ],
        )?,
        ToolDefinition::new(
            PROJECT_COMPUTATION_TOOL,
            "Run one deterministic, synchronous JavaScript computation over explicitly listed project files. `source` must be one ECMAScript module with exactly one default export: `export default function compute(zeta) { ... }`. Inside it, read a declared input only by its `id`: use `zeta.inputText(\"id\")` for `utf8`, `zeta.inputJson(\"id\")` for `json`, or `zeta.inputBytes(\"id\")` for `bytes`. Inputs are not globals, and source cannot open their paths. Example: `export default function compute(zeta) { return { bytes: zeta.inputText(\"orders\").length }; }`. Return the declared text or JSON value directly. No imports, filesystem, network, environment, clocks, randomness, processes or packages are available. After success, answer the user normally; never run another computation merely to format or explain its result.",
            computation_schema(true),
            ToolClassification::Read,
            ProjectCapability::WorkspaceRead,
            ApprovalRequirement::CapabilityOnly,
            RiskLevel::Low,
            ToolLimits::new(WALL_TIME_LIMIT, 64 * 1024, 1)?,
            [
                SupportedResultType::Text,
                SupportedResultType::Json,
                SupportedResultType::OpaqueLocalReference,
            ],
        )?,
    ])
}

fn computation_schema(project_data: bool) -> serde_json::Value {
    let output = serde_json::json!({
        "type": "object",
        "description": "Required nested output contract: keep kind, maxBytes and maxItems inside this object. These bounds cap the returned value; they are not computation inputs.",
        "properties": {
            "kind": {"type": "string", "enum": ["text", "json"]},
            "maxBytes": {"type": "integer", "minimum": 1, "maximum": MAX_OUTPUT_BYTES},
            "maxItems": {"type": "integer", "minimum": 1, "maximum": 4096}
        },
        "required": ["kind", "maxBytes", "maxItems"],
        "additionalProperties": false
    });
    let source_description = if project_data {
        "One synchronous ECMAScript module with exactly `export default function compute(zeta) { ... }`. Read declared inputs only with `zeta.inputText(id)` for utf8, `zeta.inputJson(id)` for json, or `zeta.inputBytes(id)` for bytes, then return the declared value."
    } else {
        "One synchronous ECMAScript module with exactly `export default function compute(zeta) { ... }`, returning the declared value. This tool has no inputs; put supplied values directly in source, not in zeta properties or extra arguments."
    };
    let mut properties = serde_json::Map::from_iter([
        (
            "source".to_owned(),
            serde_json::json!({
                "type": "string",
                "description": source_description,
                "minLength": 1,
                "maxLength": MAX_SOURCE_BYTES
            }),
        ),
        ("output".to_owned(), output),
    ]);
    let mut required = vec!["source", "output"];
    if project_data {
        properties.insert(
            "workspace".to_owned(),
            serde_json::json!({"type": "string", "minLength": 1, "maxLength": 80}),
        );
        properties.insert(
            "inputs".to_owned(),
            serde_json::json!({
                "type": "array",
                "minItems": 1,
                "maxItems": MAX_INPUTS,
                "items": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "Request-local name passed verbatim to the matching zeta input accessor in source.",
                            "pattern": "^[A-Za-z][A-Za-z0-9_-]{0,63}$"
                        },
                        "path": {"type": "string", "minLength": 1, "maxLength": 4096},
                        "format": {
                            "type": "string",
                            "description": "Selects the required accessor: utf8 uses zeta.inputText, json uses zeta.inputJson, and bytes uses zeta.inputBytes.",
                            "enum": ["utf8", "json", "bytes"]
                        }
                    },
                    "required": ["id", "path", "format"],
                    "additionalProperties": false
                }
            }),
        );
        required.extend(["workspace", "inputs"]);
    }
    serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registrations_distinguish_no_authority_from_workspace_read()
    -> Result<(), Box<dyn std::error::Error>> {
        let registry = computation_tool_registry()?;
        let definitions = registry.definitions().collect::<Vec<_>>();
        assert_eq!(definitions.len(), 2);
        assert_eq!(definitions[0].name(), PURE_COMPUTATION_TOOL);
        assert_eq!(
            definitions[0].capability(),
            ProjectCapability::NoProjectAuthority
        );
        assert_eq!(definitions[1].name(), PROJECT_COMPUTATION_TOOL);
        assert_eq!(
            definitions[1].capability(),
            ProjectCapability::WorkspaceRead
        );
        assert!(
            definitions[0]
                .description()
                .contains("export default function compute(zeta)")
        );
        let project_description = definitions[1].description();
        for required in [
            "export default function compute(zeta)",
            "zeta.inputText",
            "zeta.inputJson",
            "zeta.inputBytes",
            "Inputs are not globals",
            "answer the user normally",
        ] {
            assert!(project_description.contains(required), "missing {required}");
        }
        let project_schema = definitions[1].input_schema();
        let source_description = project_schema["properties"]["source"]["description"]
            .as_str()
            .ok_or("project source description is absent")?;
        assert!(source_description.contains("zeta.inputText(id)"));
        assert_eq!(
            definitions[0].limits().timeout_ms,
            u64::try_from(WALL_TIME_LIMIT.as_millis())?
        );
        Ok(())
    }
}
