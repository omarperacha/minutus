use super::*;

pub(super) fn tool(name: &str) -> PromptToolDefinition {
    PromptToolDefinition {
        name: name.to_owned(),
        description: String::new(),
        parameters: serde_json::json!({}),
    }
}

pub(super) fn tool_with_parameters(
    name: &str,
    properties: &serde_json::Value,
) -> PromptToolDefinition {
    PromptToolDefinition {
        name: name.to_owned(),
        description: String::new(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": properties,
            "additionalProperties": false
        }),
    }
}

pub(super) fn collector_tool() -> PromptToolDefinition {
    tool_with_parameters(
        "filesystem.collect_evidence",
        &serde_json::json!({
            "items": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "string"},
                        "operation": {
                            "type": "string",
                            "enum": [
                                "find_matches",
                                "load_exact_text_format",
                                "load_exact_image_format",
                                "load_exact_text_semantic",
                                "load_exact_image_semantic"
                            ]
                        }
                    },
                    "required": ["id", "operation"],
                    "additionalProperties": false
                }
            }
        }),
    )
}

pub(super) fn plan_call(mode: &str, expected_tools: &[&str]) -> ParsedToolCall {
    let expected_calls = expected_tools
        .iter()
        .map(|tool| serde_json::json!({"tool": tool, "purpose": "Complete this step"}))
        .collect::<Vec<_>>();
    ParsedToolCall {
        call_id: format!("plan-{mode}"),
        recipient: PLAN_TOOL.to_owned(),
        name: PLAN_TOOL.to_owned(),
        arguments: BTreeMap::from([
            ("assumptions".to_owned(), serde_json::json!([])),
            ("mode".to_owned(), serde_json::json!(mode)),
            ("outcome".to_owned(), serde_json::json!("other")),
            (
                "steps".to_owned(),
                serde_json::json!([{
                    "completionEvidence": "The requested artifact is verified",
                    "expectedCalls": expected_calls,
                    "id": "produce",
                    "purpose": "Inspect evidence and produce the artifact",
                    "requiredEvidence": ["Source and convention evidence"]
                }]),
            ),
            (
                "successCriteria".to_owned(),
                serde_json::json!(["The requested file exists and is verified"]),
            ),
            (
                "target".to_owned(),
                serde_json::json!("outputs/reconstructed_tables/monthly cashflow.csv"),
            ),
        ]),
    }
}

pub(super) fn stage_completion(stage_id: &str) -> ParsedToolCall {
    ParsedToolCall {
        call_id: format!("complete-{stage_id}"),
        recipient: COMPLETE_STAGE_TOOL.to_owned(),
        name: COMPLETE_STAGE_TOOL.to_owned(),
        arguments: BTreeMap::new(),
    }
}

pub(super) fn semantic_plan_call(mode: &str) -> ParsedToolCall {
    ParsedToolCall {
        call_id: format!("semantic-plan-{mode}"),
        recipient: PLAN_TOOL.to_owned(),
        name: PLAN_TOOL.to_owned(),
        arguments: BTreeMap::from([
            ("assumptions".to_owned(), serde_json::json!([])),
            (
                "constraints".to_owned(),
                serde_json::json!(["Preserve source values"]),
            ),
            (
                "contractVersion".to_owned(),
                serde_json::json!(crate::chat_tools::ROLELESS_SEMANTIC_PLAN_CONTRACT_VERSION),
            ),
            ("mode".to_owned(), serde_json::json!(mode)),
            ("outcome".to_owned(), serde_json::json!("create_file")),
            (
                "steps".to_owned(),
                serde_json::json!([
                    {
                        "actionClass": "read",
                        "dependsOn": [],
                        "doneWhen": "The source content is loaded",
                        "id": "load-source",
                        "objective": "Load the source artifact",
                        "preferredTools": ["filesystem.inspect_image"],
                        "requiredEvidence": ["semantic_content"]
                    },
                    {
                        "actionClass": "workspace_write",
                        "dependsOn": ["load-source"],
                        "doneWhen": "The requested artifact is verified",
                        "id": "write-output",
                        "objective": "Create the requested artifact",
                        "requiredEvidence": ["semantic_content"]
                    }
                ]),
            ),
            (
                "successCriteria".to_owned(),
                serde_json::json!(["The requested artifact exists and is verified"]),
            ),
            ("target".to_owned(), serde_json::json!("result/output.csv")),
        ]),
    }
}

pub(super) fn semantic_forecast_call(mode: &str) -> ParsedToolCall {
    ParsedToolCall {
        call_id: format!("semantic-forecast-{mode}"),
        recipient: FORECAST_TOOL.to_owned(),
        name: FORECAST_TOOL.to_owned(),
        arguments: BTreeMap::from([
            (
                "contractVersion".to_owned(),
                serde_json::json!(LEGACY_EXECUTION_FORECAST_CONTRACT_VERSION),
            ),
            ("mode".to_owned(), serde_json::json!(mode)),
            (
                "entries".to_owned(),
                serde_json::json!([
                    {
                        "stepId":"load-source", "category":"workspace_read",
                        "purpose":"Load source evidence", "dependsOn":[],
                        "requiredEvidence":["semantic_content"], "postcondition":"content_loaded",
                        "routes":[{"operation":"filesystem.inspect_image","knownArguments":[],"unresolvedArguments":[]}]
                    },
                    {
                        "stepId":"write-output", "category":"workspace_write",
                        "purpose":"Write verified output", "dependsOn":["load-source"],
                        "requiredEvidence":["semantic_content"], "postcondition":"file_write_verified",
                        "routes":[{"operation":"filesystem.write_text","knownArguments":[],"unresolvedArguments":[]}]
                    }
                ]),
            ),
        ]),
    }
}

pub(super) fn semantic_v3_plan_call(mode: &str) -> ParsedToolCall {
    ParsedToolCall {
        call_id: format!("semantic-v3-plan-{mode}"),
        recipient: PLAN_TOOL.to_owned(),
        name: PLAN_TOOL.to_owned(),
        arguments: BTreeMap::from([
            ("assumptions".to_owned(), serde_json::json!([])),
            (
                "constraints".to_owned(),
                serde_json::json!(["Preserve source values"]),
            ),
            (
                "contractVersion".to_owned(),
                serde_json::json!(crate::chat_tools::ROLE_AWARE_SEMANTIC_PLAN_CONTRACT_VERSION),
            ),
            ("mode".to_owned(), serde_json::json!(mode)),
            ("outcome".to_owned(), serde_json::json!("create_file")),
            (
                "steps".to_owned(),
                serde_json::json!([
                    {
                        "actionClass":"read", "dependsOn":[],
                        "doneWhen":"The exact source image content is loaded", "id":"source",
                        "objective":"Load the source image",
                        "obligations":[{"id":"source-image","roleKind":"source","evidenceClass":"semantic_content","postcondition":"image_content_loaded"}]
                    },
                    {
                        "actionClass":"read", "dependsOn":[],
                        "doneWhen":"The exact convention structure is loaded", "id":"convention",
                        "objective":"Load the convention",
                        "obligations":[{"id":"convention-format","roleKind":"convention","evidenceClass":"format_structure","postcondition":"text_format_loaded"}]
                    },
                    {
                        "actionClass":"workspace_write", "dependsOn":["source","convention"],
                        "doneWhen":"The requested artifact is verified", "id":"write",
                        "objective":"Create the requested artifact",
                        "obligations":[
                            {"id":"grounded-source","roleKind":"source","evidenceClass":"semantic_content","postcondition":"image_content_loaded"},
                            {"id":"grounded-convention","roleKind":"convention","evidenceClass":"format_structure","postcondition":"text_format_loaded"},
                            {"id":"written-target","roleKind":"target","evidenceClass":"identity_location","postcondition":"file_write_verified"}
                        ]
                    }
                ]),
            ),
            (
                "successCriteria".to_owned(),
                serde_json::json!(["The artifact exists and is verified"]),
            ),
            ("target".to_owned(), serde_json::json!("result/output.csv")),
        ]),
    }
}

pub(super) fn semantic_v3_forecast_call(mode: &str) -> ParsedToolCall {
    ParsedToolCall {
        call_id: format!("semantic-v3-forecast-{mode}"),
        recipient: FORECAST_TOOL.to_owned(),
        name: FORECAST_TOOL.to_owned(),
        arguments: BTreeMap::from([
            (
                "contractVersion".to_owned(),
                serde_json::json!(ROLE_AWARE_EXECUTION_FORECAST_CONTRACT_VERSION),
            ),
            ("mode".to_owned(), serde_json::json!(mode)),
            (
                "entries".to_owned(),
                serde_json::json!([
                    {"stepId":"source","category":"workspace_read","purpose":"Load source","dependsOn":[],"requiredObligations":["source-image"],"postcondition":"content_loaded","routes":[{"operation":"filesystem.inspect_image","knownArguments":[],"unresolvedArguments":[]}]},
                    {"stepId":"convention","category":"workspace_read","purpose":"Load convention","dependsOn":[],"requiredObligations":["convention-format"],"postcondition":"content_loaded","routes":[{"operation":"filesystem.read_text","knownArguments":[],"unresolvedArguments":[]}]},
                    {"stepId":"write","category":"workspace_write","purpose":"Write output","dependsOn":["source","convention"],"requiredObligations":["grounded-source","grounded-convention","written-target"],"postcondition":"file_write_verified","routes":[{"operation":"filesystem.write_text","knownArguments":[],"unresolvedArguments":[]}]}
                ]),
            ),
        ]),
    }
}

pub(super) fn semantic_v4_plan_call(mode: &str) -> ParsedToolCall {
    let mut call = semantic_v3_plan_call(mode);
    call.call_id = format!("semantic-v4-plan-{mode}");
    call.arguments.insert(
        "contractVersion".to_owned(),
        serde_json::json!(crate::chat_tools::OUTCOME_SEMANTIC_PLAN_CONTRACT_VERSION),
    );
    call.arguments.insert(
        "steps".to_owned(),
        serde_json::json!([
            {
                "actionClass":"read", "dependsOn":[],
                "doneWhen":"The exact source image content is loaded", "id":"source",
                "objective":"Load the source image",
                "obligations":[{"id":"source-image","kind":"image_content_loaded","roleKind":"source"}]
            },
            {
                "actionClass":"read", "dependsOn":[],
                "doneWhen":"The exact convention structure is loaded", "id":"convention",
                "objective":"Load the convention",
                "obligations":[{"id":"convention-format","kind":"text_format_loaded","roleKind":"convention"}]
            },
            {
                "actionClass":"workspace_write", "dependsOn":["source","convention"],
                "doneWhen":"The requested artifact is verified", "id":"write",
                "objective":"Create the requested artifact",
                "obligations":[
                    {"id":"grounded-source","kind":"image_content_loaded","roleKind":"source"},
                    {"id":"grounded-convention","kind":"text_format_loaded","roleKind":"convention"},
                    {"id":"written-target","kind":"file_write_verified"}
                ]
            }
        ]),
    );
    call
}

pub(super) fn semantic_v5_plan_call(mode: &str) -> ParsedToolCall {
    let mut call = semantic_v4_plan_call(mode);
    call.call_id = format!("semantic-v5-plan-{mode}");
    call.arguments.insert(
        "contractVersion".to_owned(),
        serde_json::json!(crate::chat_tools::HOST_ROUTED_SEMANTIC_PLAN_CONTRACT_VERSION),
    );
    call
}

pub(super) fn semantic_v6_plan_call(mode: &str) -> ParsedToolCall {
    ParsedToolCall {
        call_id: format!("semantic-v6-plan-{mode}"),
        recipient: PLAN_TOOL.to_owned(),
        name: PLAN_TOOL.to_owned(),
        arguments: BTreeMap::from([
            ("assumptions".to_owned(), serde_json::json!([])),
            (
                "constraints".to_owned(),
                serde_json::json!(["Preserve source values"]),
            ),
            (
                "contractVersion".to_owned(),
                serde_json::json!(crate::chat_tools::SEMANTIC_PLAN_CONTRACT_VERSION),
            ),
            ("mode".to_owned(), serde_json::json!(mode)),
            ("outcome".to_owned(), serde_json::json!("create_file")),
            (
                "steps".to_owned(),
                serde_json::json!([
                    {
                        "actionClass":"read", "dependsOn":[],
                        "doneWhen":"The source content is available", "id":"read-source",
                        "objective":"Load the source content",
                        "suggestedTools":["filesystem.read_text"]
                    },
                    {
                        "actionClass":"workspace_write", "dependsOn":["read-source"],
                        "doneWhen":"The requested file exists", "id":"write-output",
                        "objective":"Create the requested file",
                        "suggestedTools":["filesystem.write_text"]
                    }
                ]),
            ),
            (
                "successCriteria".to_owned(),
                serde_json::json!(["The requested file is created"]),
            ),
            ("target".to_owned(), serde_json::json!("result/output.txt")),
        ]),
    }
}

pub(super) fn semantic_v4_forecast_call(mode: &str) -> ParsedToolCall {
    let mut call = semantic_v3_forecast_call(mode);
    call.call_id = format!("semantic-v4-forecast-{mode}");
    call.arguments.insert(
        "contractVersion".to_owned(),
        serde_json::json!(EXECUTION_FORECAST_CONTRACT_VERSION),
    );
    call
}

pub(super) fn exact_bounded_write_plan() -> ParsedToolCall {
    let mut plan = semantic_plan_call("initial");
    plan.arguments.insert(
        "steps".to_owned(),
        serde_json::json!([
            {
                "actionClass": "read",
                "dependsOn": [],
                "doneWhen": "Input path located",
                "id": "locate",
                "objective": "Locate the input file",
                "preferredTools": ["filesystem.find_paths"],
                "requiredEvidence": ["identity_location"]
            },
            {
                "actionClass": "read",
                "dependsOn": ["locate"],
                "doneWhen": "Source text loaded",
                "id": "source",
                "objective": "Load source text",
                "preferredTools": ["filesystem.read_text"],
                "requiredEvidence": ["semantic_content"]
            },
            {
                "actionClass": "compute",
                "dependsOn": ["source"],
                "doneWhen": "Typed result completed",
                "id": "compute",
                "objective": "Compute the requested result",
                "preferredTools": ["computation.run"],
                "requiredEvidence": ["semantic_content"]
            },
            {
                "actionClass": "workspace_write",
                "dependsOn": ["compute"],
                "doneWhen": "Output file verified",
                "id": "write",
                "objective": "Write the requested output",
                "preferredTools": ["filesystem.write_text"],
                "requiredEvidence": ["semantic_content"]
            }
        ]),
    );
    plan
}

pub(super) fn plan_with_step_count(count: usize) -> ParsedToolCall {
    let mut call = plan_call("initial", &["filesystem.read_text"]);
    call.arguments.insert(
        "steps".to_owned(),
        serde_json::Value::Array(
            (0..count)
                .map(|index| {
                    serde_json::json!({
                        "completionEvidence": format!("Step {index} is complete"),
                        "expectedCalls": [{
                            "purpose": "Read the evidence required by this step",
                            "tool": "filesystem.read_text"
                        }],
                        "id": format!("step-{index}"),
                        "purpose": format!("Complete step {index}"),
                        "requiredEvidence": ["Relevant workspace evidence"]
                    })
                })
                .collect(),
        ),
    );
    call
}

pub(super) fn standard_step_progress(
    ordinal: usize,
    status: ToolPlanStepStatus,
    remaining_tools: Vec<String>,
    completed_call_ids: Vec<String>,
) -> ToolPlanStepProgress {
    let index = ordinal.saturating_sub(1);
    ToolPlanStepProgress::new(ordinal, status, remaining_tools)
        .with_contract(
            format!("step-{index}"),
            format!("Complete step {index}"),
            vec!["Relevant workspace evidence".to_owned()],
            format!("Step {index} is complete"),
            completed_call_ids,
        )
        .with_dependencies(if index > 0 {
            vec![format!("step-{}", index.saturating_sub(1))]
        } else {
            Vec::new()
        })
}

pub(super) fn operation(id: &str, name: &str) -> ParsedToolCall {
    ParsedToolCall {
        call_id: id.to_owned(),
        recipient: name.to_owned(),
        name: name.to_owned(),
        arguments: BTreeMap::new(),
    }
}

pub(super) fn typed_collector_operation(id: &str, loads_content: bool) -> ParsedToolCall {
    let mut call = operation(id, "filesystem.collect_evidence");
    call.arguments.insert(
        "items".to_owned(),
        if loads_content {
            serde_json::json!([
                {"id":"source","operation":"load_exact_image_semantic"},
                {"id":"convention","operation":"load_exact_text_format"}
            ])
        } else {
            serde_json::json!([{
                "id":"candidates","operation":"find_matches"
            }])
        },
    );
    call
}

pub(super) fn artifact_operation(
    id: &str,
    source_call_id: &str,
    convention_call_id: &str,
) -> ParsedToolCall {
    let mut call = operation(id, "orchestration.transform_artifact");
    call.arguments.insert(
        "evidence".to_owned(),
        serde_json::json!([
            {"callId": source_call_id, "role": "source"},
            {"callId": convention_call_id, "role": "convention"}
        ]),
    );
    call
}

pub(super) fn scoped_operation(
    id: &str,
    name: &str,
    argument: &str,
    value: &str,
) -> ParsedToolCall {
    let mut call = operation(id, name);
    call.arguments.insert(
        argument.to_owned(),
        serde_json::Value::String(value.to_owned()),
    );
    call
}
