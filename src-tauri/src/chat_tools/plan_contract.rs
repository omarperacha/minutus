//! Model-visible semantic execution-plan schema.

use model_runtime::PromptToolDefinition;

use super::MAX_PROVISIONAL_PLAN_STEPS;

pub(crate) const LEGACY_SEMANTIC_PLAN_CONTRACT_VERSION: &str = "semantic_execution_v1";
pub(crate) const ROLELESS_SEMANTIC_PLAN_CONTRACT_VERSION: &str = "semantic_execution_v2";
pub(crate) const ROLE_AWARE_SEMANTIC_PLAN_CONTRACT_VERSION: &str = "semantic_execution_v3";
pub(crate) const OUTCOME_SEMANTIC_PLAN_CONTRACT_VERSION: &str = "semantic_execution_v4";
pub(crate) const HOST_ROUTED_SEMANTIC_PLAN_CONTRACT_VERSION: &str = "semantic_execution_v5";
pub(crate) const SEMANTIC_PLAN_CONTRACT_VERSION: &str = "semantic_execution_v6";

fn advisory_step_schema(forecastable_tools: &[String]) -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "actionClass": { "type": "string", "enum": ["read", "compute", "workspace_write", "external_write"], "description": "Broad intended work in this stage. A multi-operation tool may support work in different classes; this label grants no execution authority." },
            "dependsOn": { "type": "array", "minItems": 0, "maxItems": 15, "uniqueItems": true, "description": "Optional declared dependencies as a native JSON array of stage-id strings. Omission or [] means no declared dependencies; never stringify the array.", "items": { "type": "string", "minLength": 1, "maxLength": 32 } },
            "doneWhen": { "type": "string", "minLength": 1, "maxLength": 256 },
            "id": { "type": "string", "minLength": 1, "maxLength": 32, "description": "Stable task-local stage identity. Retain it when clarifying the same work: read/compute completion survives wording changes with the same action class and dependencies. Use a new ID and update dependent IDs to reopen or replace completed work. Changed effect requirements must be verified again." },
            "objective": { "type": "string", "minLength": 1, "maxLength": 256 },
            "suggestedTools": {
                "type": "array", "minItems": 1, "maxItems": 8, "uniqueItems": true,
                "description": "Available tools that can advance this objective. Suggestions load schemas; they are advisory and grant no permission. Use a native JSON array, never a stringified array.",
                "items": { "type": "string", "enum": forecastable_tools }
            }
        },
        "required": ["id", "objective", "doneWhen", "actionClass", "suggestedTools"],
        "additionalProperties": false
    })
}

pub(super) fn tool_definition(forecastable_tools: &[String]) -> PromptToolDefinition {
    PromptToolDefinition {
        name: "orchestration.submit_plan".to_owned(),
        description: format!(
            "Submit a concise semantic_execution_v6 plan alone before broader task operations. Available bounded discovery may precede planning dependent work. Name useful available tools in suggestedTools; acceptance loads their complete schemas. Plan only work that is known now, up to {MAX_PROVISIONAL_PLAN_STEPS} stages. Discover missing facts before planning dependent detail. Asking the user for information and reporting results are ordinary dialogue, not tool stages. Omit private cognition and unrequested verification stages; image transcription happens when forming the consumer call after loading the source. Include each tool-backed objective, dependencies and observable doneWhen condition. actionClass describes intended work, not a tool's maximum authority. Suggested tools are advisory; use orchestration.search_tools to load a better tool without revising the plan. Revise only when new evidence materially changes the task or stage work, not to record completed results or change tool preferences. Use native JSON arrays for all plural fields, never quoted or JSON-stringified arrays. An accepted plan grants no permissions or approvals; actual operations retain their own checks."
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "assumptions": {
                    "type": "array", "minItems": 0, "maxItems": 6,
                    "description": "A native JSON array of assumption strings; use [] when empty and never stringify the array.",
                    "items": { "type": "string", "minLength": 1, "maxLength": 256 }
                },
                "constraints": {
                    "type": "array", "minItems": 0, "maxItems": 8,
                    "description": "A native JSON array of hard task-constraint strings; use [] when empty and never stringify the array. This field cannot grant authority.",
                    "items": { "type": "string", "minLength": 1, "maxLength": 256 }
                },
                "contractVersion": {
                    "type": "string", "enum": [SEMANTIC_PLAN_CONTRACT_VERSION]
                },
                "mode": { "type": "string", "enum": ["initial", "revise"] },
                "outcome": { "type": "string", "enum": ["answer", "create_file", "modify_file", "other"], "description": "answer for information or dialogue; create_file/modify_file only for a requested workspace file; other for external transactions or other effects." },
                "steps": {
                    "type": "array", "minItems": 1, "maxItems": MAX_PROVISIONAL_PLAN_STEPS,
                    "description": "A native JSON array of stage objects, never a quoted or JSON-stringified array. Include tool-backed effects and observable boundaries, not private cognition. suggestedTools guide discovery and selection without restricting execution.",
                    "items": advisory_step_schema(forecastable_tools)
                },
                "successCriteria": {
                    "type": "array", "minItems": 1, "maxItems": 8,
                    "description": "A native JSON array of observable success-criterion strings, never a quoted or JSON-stringified array.",
                    "items": { "type": "string", "minLength": 1, "maxLength": 256 }
                },
                "target": { "type": "string", "minLength": 1, "maxLength": 512 }
            },
            "required": ["contractVersion", "mode", "outcome", "target", "constraints", "successCriteria", "assumptions", "steps"],
            "additionalProperties": false
        }),
    }
}

pub(super) fn stage_completion_tool_definition() -> PromptToolDefinition {
    PromptToolDefinition {
        name: "orchestration.complete_stage".to_owned(),
        description: "Declare that the single current semantic_execution_v6 stage's doneWhen condition is satisfied. This is the model's advisory semantic judgment, takes no arguments, and may be called alone or before dependency-ready successor work in the same response. The host owns the current stage cursor. Avoid a separate round solely to acknowledge progress; when a successor task call is ready, include it after this control. Ask missing-input questions without housekeeping controls. One valid checkpoint per batch does not consume the task-call allowance; round limits still apply. Other calls, including revision and tool search, consume the call allowance. Do not batch this with another orchestration control. Completion grants no permission and cannot replace host verification of an effect or final outcome. Use optional plan revision only when the target, constraints, success criteria, stage work, dependencies, or action-risk class materially changed.".to_owned(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {},
            "required": [],
            "additionalProperties": false
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_schema_promotes_tools_without_obligations() -> Result<(), ipc_types::IpcError> {
        let definition = tool_definition(&[
            "filesystem.read_text".to_owned(),
            "filesystem.write_text".to_owned(),
        ]);
        let step = &definition.parameters["properties"]["steps"]["items"];
        let required = step["required"]
            .as_array()
            .ok_or_else(ipc_types::IpcError::internal)?;
        assert!(required.iter().any(|field| field == "suggestedTools"));
        assert!(!required.iter().any(|field| field == "requiredEvidence"));
        assert!(!required.iter().any(|field| field == "requires"));
        assert!(!required.iter().any(|field| field == "produces"));
        assert!(!required.iter().any(|field| field == "obligations"));
        assert!(step["properties"].get("preferredTools").is_none());
        let schema = definition.parameters.to_string();
        assert!(!schema.contains("evidenceClass"));
        assert!(!schema.contains("postcondition"));
        assert!(schema.contains("filesystem.read_text"));
        assert!(!schema.contains("file_write_verified"));
        assert!(schema.contains("native JSON array"));
        assert!(schema.contains("never stringify"));
        assert!(schema.contains("Use a new ID"));
        assert!(
            definition
                .description
                .contains("orchestration.search_tools")
        );
        assert!(
            definition
                .description
                .contains("never quoted or JSON-stringified")
        );
        Ok(())
    }

    #[test]
    fn semantic_suggestions_are_available_independently_of_stage_labels() {
        let tools = ["filesystem.read_text".to_owned(), "x402.request".to_owned()];
        let definition = tool_definition(&tools);
        let stage = &definition.parameters["properties"]["steps"]["items"];
        assert_eq!(
            stage["properties"]["suggestedTools"]["items"]["enum"],
            serde_json::json!(tools)
        );
        assert_eq!(
            stage["properties"]["actionClass"]["enum"],
            serde_json::json!(["read", "compute", "workspace_write", "external_write"])
        );
        let read_only = tool_definition(&["filesystem.read_text".to_owned()]);
        assert!(!read_only.parameters.to_string().contains("x402.request"));
        assert!(
            !read_only
                .parameters
                .to_string()
                .contains("filesystem.write_text")
        );
        assert!(definition.description.contains("ordinary dialogue"));
    }

    #[test]
    fn stage_completion_schema_is_small_and_explicit() {
        let definition = stage_completion_tool_definition();
        assert_eq!(definition.name, "orchestration.complete_stage");
        assert_eq!(definition.parameters["required"], serde_json::json!([]));
        assert_eq!(definition.parameters["additionalProperties"], false);
        assert!(
            definition
                .description
                .contains("model's advisory semantic judgment")
        );
        assert!(definition.description.contains("may be called alone"));
        assert!(definition.description.contains("grants no permission"));
        assert!(
            definition
                .description
                .contains("does not consume the task-call allowance")
        );
        assert_eq!(definition.parameters["properties"], serde_json::json!({}));
    }
}
