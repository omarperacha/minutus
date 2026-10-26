use super::*;

pub(super) fn completed_operation(id: &str, name: ToolName) -> ChatToolExchange {
    let call_id = ToolCallId(id.to_owned());
    let content = if name == ToolName::FilesystemCollectEvidence {
        vec![ipc_types::ToolResultPart::Json {
            value: ipc_types::JsonValue::Object(BTreeMap::from([
                (
                    "coverageComplete".to_owned(),
                    ipc_types::JsonValue::Bool(true),
                ),
                ("ready".to_owned(), ipc_types::JsonValue::Bool(true)),
            ])),
        }]
    } else if matches!(
        name,
        ToolName::FilesystemWriteText
            | ToolName::FilesystemReplaceText
            | ToolName::FilesystemWriteDelimitedTable
            | ToolName::OrchestrationTransformArtifact
    ) {
        vec![ipc_types::ToolResultPart::Json {
            value: ipc_types::JsonValue::Object(BTreeMap::from([
                (
                    "changes".to_owned(),
                    ipc_types::JsonValue::Array(vec![ipc_types::JsonValue::Object(
                        BTreeMap::from([(
                            "postWriteVerified".to_owned(),
                            ipc_types::JsonValue::Bool(true),
                        )]),
                    )]),
                ),
                (
                    "postconditionVerified".to_owned(),
                    ipc_types::JsonValue::Bool(true),
                ),
            ])),
        }]
    } else if name == ToolName::FilesystemReadText {
        vec![ipc_types::ToolResultPart::Json {
            value: ipc_types::JsonValue::Object(BTreeMap::from([(
                "text".to_owned(),
                ipc_types::JsonValue::String("bounded fixture".to_owned()),
            )])),
        }]
    } else if matches!(
        name,
        ToolName::ComputationRun | ToolName::ComputationRunProject
    ) {
        vec![ipc_types::ToolResultPart::Json {
            value: ipc_types::JsonValue::Object(BTreeMap::from([(
                "status".to_owned(),
                ipc_types::JsonValue::String("completed".to_owned()),
            )])),
        }]
    } else {
        Vec::new()
    };
    let inspected_asset_id = (name == ToolName::FilesystemInspectImage)
        .then(|| ipc_types::AssetId(format!("asset-{id}")));
    let computation_reuse_digest = matches!(
        name,
        ToolName::ComputationRun | ToolName::ComputationRunProject
    )
    .then(|| format!("digest-{id}"));
    ChatToolExchange {
        request: ToolRequest {
            call_id: call_id.clone(),
            name,
            arguments: BTreeMap::new(),
        },
        result: ToolResult {
            call_id,
            outcome: ToolOutcome::Completed { content },
        },
        activity: TaskActivity {
            id: id.to_owned(),
            kind: TaskActivityKind::FileWrite,
            title: "operation".to_owned(),
            status: TaskActivityStatus::Completed,
            detail: None,
        },
        workspace_grant_id: Some(WorkspaceGrantId("grant".to_owned())),
        undo_changes: Vec::new(),
        inspected_asset_id,
        computation_reuse_digest,
    }
}

pub(super) fn completed_typed_collector_operation(id: &str) -> ChatToolExchange {
    let mut exchange = completed_operation(id, ToolName::FilesystemCollectEvidence);
    exchange.inspected_asset_id = Some(ipc_types::AssetId(format!("asset-{id}")));
    exchange.result.outcome = ToolOutcome::Completed {
        content: vec![ToolResultPart::Json {
            value: JsonValue::Object(BTreeMap::from([
                ("coverageComplete".to_owned(), JsonValue::Bool(true)),
                ("ready".to_owned(), JsonValue::Bool(true)),
                (
                    "items".to_owned(),
                    JsonValue::Array(vec![
                        JsonValue::Object(BTreeMap::from([
                            (
                                "id".to_owned(),
                                JsonValue::String("source-image".to_owned()),
                            ),
                            ("status".to_owned(), JsonValue::String("ready".to_owned())),
                            ("image".to_owned(), JsonValue::Object(BTreeMap::new())),
                        ])),
                        JsonValue::Object(BTreeMap::from([
                            (
                                "id".to_owned(),
                                JsonValue::String("convention-format".to_owned()),
                            ),
                            ("status".to_owned(), JsonValue::String("ready".to_owned())),
                            (
                                "text".to_owned(),
                                JsonValue::String("header,value".to_owned()),
                            ),
                        ])),
                    ]),
                ),
            ])),
        }],
    };
    exchange
}

pub(super) fn incomplete_evidence_operation(id: &str) -> ChatToolExchange {
    let mut exchange = completed_operation(id, ToolName::FilesystemCollectEvidence);
    exchange.result.outcome = ToolOutcome::Completed {
        content: vec![ipc_types::ToolResultPart::Json {
            value: ipc_types::JsonValue::Object(BTreeMap::from([
                (
                    "coverageComplete".to_owned(),
                    ipc_types::JsonValue::Bool(false),
                ),
                ("ready".to_owned(), ipc_types::JsonValue::Bool(false)),
            ])),
        }],
    };
    exchange
}

pub(super) fn collector_artifact_plan() -> ParsedToolCall {
    let mut plan = plan_call("initial", &["orchestration.transform_artifact"]);
    plan.arguments
        .insert("outcome".to_owned(), serde_json::json!("create_file"));
    plan.arguments.insert(
        "steps".to_owned(),
        serde_json::json!([
            {
                "completionEvidence":"Source and convention are loaded",
                "expectedCalls":[{
                    "purpose":"Load exact source and convention",
                    "providesEvidence":[
                        {"role":"source","evidenceClass":"semantic_content"},
                        {"role":"convention","evidenceClass":"format_structure"}
                    ],
                    "tool":"filesystem.collect_evidence"
                }],
                "id":"evidence", "purpose":"Collect evidence",
                "requiredEvidence":["Source and convention"]
            },
            {
                "completionEvidence":"Artifact is verified",
                "expectedCalls":[{"purpose":"Create artifact","tool":"orchestration.transform_artifact"}],
                "id":"artifact", "purpose":"Create artifact",
                "requiredEvidence":["Typed evidence"]
            }
        ]),
    );
    plan
}

pub(super) fn incomplete_collector_projection() -> ToolPlanProgress {
    ToolPlanProgress::new(
        1,
        0,
        vec![
            ToolPlanStepProgress::new(
                1,
                ToolPlanStepStatus::Blocked,
                vec!["filesystem.collect_evidence".to_owned()],
            )
            .with_contract(
                "evidence".to_owned(),
                "Collect evidence".to_owned(),
                vec!["Source and convention".to_owned()],
                "Source and convention are loaded".to_owned(),
                Vec::new(),
            ),
            ToolPlanStepProgress::new(
                2,
                ToolPlanStepStatus::NotStarted,
                vec!["orchestration.transform_artifact".to_owned()],
            )
            .with_contract(
                "artifact".to_owned(),
                "Create artifact".to_owned(),
                vec!["Typed evidence".to_owned()],
                "Artifact is verified".to_owned(),
                Vec::new(),
            )
            .with_dependencies(vec!["evidence".to_owned()]),
        ],
        Some(1),
        ToolPlanOutcome::CreateFile,
        false,
        false,
        4,
        true,
        0,
    )
    .with_retained_call_ids(["evidence-incomplete".to_owned()])
}

pub(super) fn completed_find_paths_operation(
    id: &str,
    resolutions: &[(usize, bool)],
) -> ChatToolExchange {
    let mut exchange = completed_operation(id, ToolName::FilesystemFindPaths);
    exchange.result.outcome = ToolOutcome::Completed {
        content: vec![ipc_types::ToolResultPart::Json {
            value: ipc_types::JsonValue::Object(BTreeMap::from([(
                "results".to_owned(),
                ipc_types::JsonValue::Array(
                    resolutions
                        .iter()
                        .map(|(matches, complete)| {
                            ipc_types::JsonValue::Object(BTreeMap::from([
                                ("complete".to_owned(), ipc_types::JsonValue::Bool(*complete)),
                                (
                                    "matches".to_owned(),
                                    ipc_types::JsonValue::Array(
                                        (0..*matches)
                                            .map(|index| {
                                                ipc_types::JsonValue::String(format!(
                                                    "candidate-{index}"
                                                ))
                                            })
                                            .collect(),
                                    ),
                                ),
                            ]))
                        })
                        .collect(),
                ),
            )])),
        }],
    };
    exchange
}

pub(super) fn failed_operation(id: &str, name: ToolName) -> ChatToolExchange {
    let call_id = ToolCallId(id.to_owned());
    ChatToolExchange {
        request: ToolRequest {
            call_id: call_id.clone(),
            name,
            arguments: BTreeMap::new(),
        },
        result: ToolResult {
            call_id,
            outcome: ToolOutcome::Failed {
                error: IpcError::invalid_request("bounded fixture failure"),
            },
        },
        activity: TaskActivity {
            id: id.to_owned(),
            kind: TaskActivityKind::FileRead,
            title: "operation".to_owned(),
            status: TaskActivityStatus::Partial,
            detail: None,
        },
        workspace_grant_id: Some(WorkspaceGrantId("grant".to_owned())),
        undo_changes: Vec::new(),
        inspected_asset_id: None,
        computation_reuse_digest: None,
    }
}
