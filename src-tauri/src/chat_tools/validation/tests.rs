use super::*;
use crate::chat_tools::{chat_tool_definitions, plan_contract};

#[test]
fn closed_contract_errors_name_missing_and_unexpected_plan_fields() {
    let missing_outcome = ParsedToolCall {
        call_id: "plan-missing-outcome".to_owned(),
        recipient: "orchestration.submit_plan".to_owned(),
        name: "orchestration.submit_plan".to_owned(),
        arguments: BTreeMap::from([
            ("assumptions".to_owned(), serde_json::json!([])),
            ("constraints".to_owned(), serde_json::json!([])),
            (
                "contractVersion".to_owned(),
                serde_json::json!("semantic_execution_v1"),
            ),
            ("mode".to_owned(), serde_json::json!("initial")),
            ("steps".to_owned(), serde_json::json!([])),
            ("successCriteria".to_owned(), serde_json::json!(["done"])),
            ("target".to_owned(), serde_json::json!("artifact.csv")),
        ]),
    };
    let error = validated_chat_tool_request(&missing_outcome, &[])
        .err()
        .unwrap_or_else(IpcError::internal);
    assert!(error.message.contains("orchestration.submit_plan"));
    assert!(
        error
            .message
            .contains("missing required field(s): `outcome`")
    );

    let mut unexpected = missing_outcome;
    unexpected
        .arguments
        .insert("outcome".to_owned(), serde_json::json!("create_file"));
    unexpected
        .arguments
        .insert("report".to_owned(), serde_json::json!(true));
    let error = validated_chat_tool_request(&unexpected, &[])
        .err()
        .unwrap_or_else(IpcError::internal);
    assert!(error.message.contains("unexpected field(s): `report`"));
}

#[test]
fn stage_completion_accepts_only_the_empty_v6_call() -> Result<(), IpcError> {
    let advertised = plan_contract::stage_completion_tool_definition();
    assert_eq!(advertised.parameters["required"], serde_json::json!([]));
    let mut call = ParsedToolCall {
        call_id: "complete-stage".to_owned(),
        recipient: "orchestration.complete_stage".to_owned(),
        name: "orchestration.complete_stage".to_owned(),
        arguments: BTreeMap::new(),
    };
    let request = validated_chat_tool_request(&call, &[])?;
    assert_eq!(request.name, ToolName::OrchestrationCompleteStage);
    assert!(request.arguments.is_empty());

    call.arguments
        .insert("stageId".to_owned(), serde_json::json!("locate"));
    let error = validated_chat_tool_request(&call, &[])
        .err()
        .ok_or_else(IpcError::internal)?;
    assert!(error.message.contains("unexpected field(s): `stageId`"));
    Ok(())
}
#[test]
fn workspace_image_inspection_requires_a_valid_project_file() -> Result<(), IpcError> {
    let workspaces = vec![ChatToolWorkspace {
        alias: "primary".to_owned(),
        display_name: "zeta-dev".to_owned(),
        grant_id: WorkspaceGrantId("grant".to_owned()),
        permission_level: WorkspacePermissionLevel::ReadOnly,
    }];
    let mut inspect = ParsedToolCall {
        call_id: "call-image".to_owned(),
        recipient: "filesystem.inspect_image".to_owned(),
        name: "filesystem.inspect_image".to_owned(),
        arguments: BTreeMap::from([
            ("path".to_owned(), serde_json::json!("diagram.png")),
            ("workspace".to_owned(), serde_json::json!("primary")),
        ]),
    };
    assert_eq!(
        validated_chat_tool_request(&inspect, &workspaces)?.name,
        ToolName::FilesystemInspectImage
    );
    inspect
        .arguments
        .insert("path".to_owned(), serde_json::json!("."));
    assert!(validated_chat_tool_request(&inspect, &workspaces).is_err());
    Ok(())
}

#[test]
fn batched_path_discovery_uses_a_closed_validated_schema() -> Result<(), IpcError> {
    let workspaces = vec![ChatToolWorkspace {
        alias: "primary".to_owned(),
        display_name: "zeta-dev".to_owned(),
        grant_id: WorkspaceGrantId("grant".to_owned()),
        permission_level: WorkspacePermissionLevel::ReadOnly,
    }];
    let call = ParsedToolCall {
        call_id: "call-find".to_owned(),
        recipient: "filesystem.find_paths".to_owned(),
        name: "filesystem.find_paths".to_owned(),
        arguments: BTreeMap::from([
            (
                "queries".to_owned(),
                serde_json::json!([{
                    "path": ".",
                    "nameContains": "monthly cashflow",
                    "extensions": ["png"],
                    "kind": "file",
                    "maxDepth": 8
                }, {
                    "path": ".",
                    "nameContains": "manifest",
                    "extensions": ["json"],
                    "kind": "file",
                    "maxDepth": 8
                }]),
            ),
            ("workspace".to_owned(), serde_json::json!("primary")),
        ]),
    };

    assert_eq!(
        validated_chat_tool_request(&call, &workspaces)?.name,
        ToolName::FilesystemFindPaths
    );

    let minimal = ParsedToolCall {
        call_id: "call-find-minimal".to_owned(),
        recipient: "filesystem.find_paths".to_owned(),
        name: "filesystem.find_paths".to_owned(),
        arguments: BTreeMap::from([
            (
                "queries".to_owned(),
                serde_json::json!([{"nameContains": "reconstructed"}]),
            ),
            ("workspace".to_owned(), serde_json::json!("primary")),
        ]),
    };
    assert_eq!(
        validated_chat_tool_request(&minimal, &workspaces)?.name,
        ToolName::FilesystemFindPaths
    );
    let resolved = required_find_path_queries_argument(&minimal.arguments)?;
    assert_eq!(resolved[0].relative_path, ".");
    assert!(resolved[0].extensions.is_empty());
    assert_eq!(resolved[0].kind, FilesystemFindPathKind::Any);
    assert_eq!(resolved[0].max_depth, DEFAULT_FIND_PATH_DEPTH);

    let mut unknown = minimal;
    unknown.arguments.insert(
        "queries".to_owned(),
        serde_json::json!([{"nameContains": "manifest", "absolutePath": "/tmp"}]),
    );
    assert!(validated_chat_tool_request(&unknown, &workspaces).is_err());
    Ok(())
}

#[test]
fn evidence_collection_is_bounded_and_defaults_safe_selectors() -> Result<(), IpcError> {
    let workspaces = vec![ChatToolWorkspace {
        alias: "primary".to_owned(),
        display_name: "project".to_owned(),
        grant_id: WorkspaceGrantId("grant".to_owned()),
        permission_level: WorkspacePermissionLevel::ReadAndWrite,
    }];
    let mut call = ParsedToolCall {
        call_id: "call-collect".to_owned(),
        recipient: "filesystem.collect_evidence".to_owned(),
        name: "filesystem.collect_evidence".to_owned(),
        arguments: BTreeMap::from([
            (
                "items".to_owned(),
                serde_json::json!([
                    {"id": "source", "path": "monthly cashflow/monthly cashflow.png", "requirement": "Transcribe the source table", "operation": "load_exact_image_semantic"},
                    {"id": "convention", "path": "outputs/manifest.json", "requirement": "Derive output naming and storage", "operation": "load_exact_text_format"}
                ]),
            ),
            ("workspace".to_owned(), serde_json::json!("primary")),
        ]),
    };
    assert_eq!(
        validated_chat_tool_request(&call, &workspaces)?.name,
        ToolName::FilesystemCollectEvidence
    );
    assert!(collect_evidence::requests_image(&call.arguments)?);

    call.arguments.insert(
            "items".to_owned(),
            serde_json::json!([
                {"id": "duplicate", "path": "a.png", "requirement": "Read A", "operation": "load_exact_image_semantic"},
                {"id": "duplicate", "path": "b.png", "requirement": "Read B", "operation": "load_exact_image_semantic"}
            ]),
        );
    assert!(validated_chat_tool_request(&call, &workspaces).is_err());
    Ok(())
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one contract test exercises read, mutation, scope, and unknown-field admission"
)]
fn planned_mutations_do_not_override_workspace_permission_or_scope() -> Result<(), IpcError> {
    let workspaces = vec![ChatToolWorkspace {
        alias: "primary".to_owned(),
        display_name: "zeta-dev".to_owned(),
        grant_id: WorkspaceGrantId("grant".to_owned()),
        permission_level: WorkspacePermissionLevel::ReadAndWrite,
    }];
    let valid = ParsedToolCall {
        call_id: "call-1".to_owned(),
        recipient: "filesystem.stat".to_owned(),
        name: "filesystem.stat".to_owned(),
        arguments: BTreeMap::from([
            ("path".to_owned(), serde_json::json!("Cargo.toml")),
            ("workspace".to_owned(), serde_json::json!("primary")),
        ]),
    };
    assert_eq!(
        validated_chat_tool_request(&valid, &workspaces)?.name,
        ToolName::FilesystemStat
    );
    let scan = ParsedToolCall {
        call_id: "call-2".to_owned(),
        recipient: "filesystem.scan".to_owned(),
        name: "filesystem.scan".to_owned(),
        arguments: BTreeMap::from([
            ("excludeExtensions".to_owned(), serde_json::json!(["md"])),
            ("minimumLineCount".to_owned(), serde_json::json!(600)),
            ("path".to_owned(), serde_json::json!(".")),
            ("recursive".to_owned(), serde_json::json!(true)),
            ("workspace".to_owned(), serde_json::json!("primary")),
        ]),
    };
    assert_eq!(
        validated_chat_tool_request(&scan, &workspaces)?.name,
        ToolName::FilesystemScan
    );
    let write = ParsedToolCall {
        call_id: "call-3".to_owned(),
        recipient: "filesystem.write_text".to_owned(),
        name: "filesystem.write_text".to_owned(),
        arguments: BTreeMap::from([
            (
                "changes".to_owned(),
                serde_json::json!([{
                    "path": "src/lib.rs",
                    "expectedSha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                    "content": "pub fn updated() {}\n",
                    "createMissingParents": false
                }]),
            ),
            ("workspace".to_owned(), serde_json::json!("primary")),
        ]),
    };
    assert_eq!(
        validated_chat_tool_request(&write, &workspaces)?.name,
        ToolName::FilesystemWriteText
    );
    let replace = ParsedToolCall {
        call_id: "call-4".to_owned(),
        recipient: "filesystem.replace_text".to_owned(),
        name: "filesystem.replace_text".to_owned(),
        arguments: BTreeMap::from([
            (
                "expectedSha256".to_owned(),
                serde_json::json!(
                    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                ),
            ),
            ("path".to_owned(), serde_json::json!("cv.html")),
            (
                "replacements".to_owned(),
                serde_json::json!([{
                    "oldText": "Old subheader",
                    "newText": "ZETA Dev | Real G",
                    "expectedOccurrences": 1
                }]),
            ),
            ("workspace".to_owned(), serde_json::json!("primary")),
        ]),
    };
    assert_eq!(
        validated_chat_tool_request(&replace, &workspaces)?.name,
        ToolName::FilesystemReplaceText
    );
    let mut read_only_workspaces = workspaces.clone();
    read_only_workspaces[0].permission_level = WorkspacePermissionLevel::ReadOnly;
    assert_eq!(chat_tool_definitions(&read_only_workspaces)?.len(), 19);
    for mutation in [&write, &replace] {
        let Err(error) = validated_chat_tool_request(mutation, &read_only_workspaces) else {
            return Err(IpcError::internal());
        };
        assert_eq!(error.code, ipc_types::IpcErrorCode::PermissionDenied);
    }

    let mut unknown_workspace = valid.clone();
    unknown_workspace
        .arguments
        .insert("workspace".to_owned(), serde_json::json!("additional-9"));
    let Err(error) = validated_chat_tool_request(&unknown_workspace, &workspaces) else {
        return Err(IpcError::internal());
    };
    assert_eq!(error.code, ipc_types::IpcErrorCode::ScopeCorrection);

    let mut extra_argument = valid;
    extra_argument
        .arguments
        .insert("absolutePath".to_owned(), serde_json::json!("/tmp/escape"));
    assert!(validated_chat_tool_request(&extra_argument, &workspaces).is_err());
    Ok(())
}

#[test]
fn delimited_table_call_uses_exact_flat_fields_and_rejects_semantic_aliases() -> Result<(), IpcError>
{
    let workspaces = vec![ChatToolWorkspace {
        alias: "primary".to_owned(),
        display_name: "workspace".to_owned(),
        grant_id: WorkspaceGrantId("grant".to_owned()),
        permission_level: WorkspacePermissionLevel::ReadAndWrite,
    }];
    let mut call = ParsedToolCall {
        call_id: "table".to_owned(),
        recipient: "filesystem.write_delimited_table".to_owned(),
        name: "filesystem.write_delimited_table".to_owned(),
        arguments: BTreeMap::from([
            (
                "columns".to_owned(),
                serde_json::json!(["period", "amount"]),
            ),
            ("delimiter".to_owned(), serde_json::json!("comma")),
            ("expectedSha256".to_owned(), serde_json::Value::Null),
            ("path".to_owned(), serde_json::json!("outputs/example.csv")),
            ("rows".to_owned(), serde_json::json!([["Example", 100]])),
            ("workspace".to_owned(), serde_json::json!("primary")),
        ]),
    };
    assert_eq!(
        validated_chat_tool_request(&call, &workspaces)?.name,
        ToolName::FilesystemWriteDelimitedTable
    );

    call.arguments.remove("delimiter");
    call.arguments
        .insert("variant".to_owned(), serde_json::json!("comma"));
    let error = validated_chat_tool_request(&call, &workspaces)
        .err()
        .ok_or_else(IpcError::internal)?;
    assert!(
        error
            .message
            .contains("missing required field(s): `delimiter`")
    );
    assert!(error.message.contains("unexpected field(s): `variant`"));
    Ok(())
}

#[test]
fn delimited_table_repair_reports_every_top_level_and_row_defect() -> Result<(), IpcError> {
    let workspaces = vec![ChatToolWorkspace {
        alias: "primary".to_owned(),
        display_name: "workspace".to_owned(),
        grant_id: WorkspaceGrantId("grant".to_owned()),
        permission_level: WorkspacePermissionLevel::ReadAndWrite,
    }];
    let catalogue = chat_tool_definitions(&workspaces)?;
    let wrapped = ParsedToolCall {
        call_id: "wrapped-table".to_owned(),
        recipient: "filesystem.write_delimited_table".to_owned(),
        name: "filesystem.write_delimited_table".to_owned(),
        arguments: BTreeMap::from([
            (
                "changes".to_owned(),
                serde_json::json!("{\"columns\":[\"period\"],\"rows\":[[\"Q1\"]]}"),
            ),
            ("workspace".to_owned(), serde_json::json!("primary")),
        ]),
    };
    let base = validated_chat_tool_request(&wrapped, &workspaces)
        .err()
        .ok_or_else(IpcError::internal)?;
    let repair = exact_argument_repair_error(&wrapped, &catalogue, &base);
    for path in [
        "$.columns",
        "$.delimiter",
        "$.expectedSha256",
        "$.path",
        "$.rows",
        "$.changes",
    ] {
        assert!(
            repair.message.contains(path),
            "missing diagnostic for {path}"
        );
    }
    assert!(repair.message.contains("native JSON arrays"));
    assert!(repair.message.contains("Repair every reported defect"));

    let rows = ParsedToolCall {
        call_id: "bad-rows".to_owned(),
        recipient: "filesystem.write_delimited_table".to_owned(),
        name: "filesystem.write_delimited_table".to_owned(),
        arguments: BTreeMap::from([
            (
                "columns".to_owned(),
                serde_json::json!(["period", "amount"]),
            ),
            ("delimiter".to_owned(), serde_json::json!("comma")),
            ("expectedSha256".to_owned(), serde_json::Value::Null),
            ("path".to_owned(), serde_json::json!("outputs/example.csv")),
            (
                "rows".to_owned(),
                serde_json::json!([["Q1", 100, "extra"], ["Q2"], ["Q3", true]]),
            ),
            ("workspace".to_owned(), serde_json::json!("primary")),
        ]),
    };
    let base = validated_chat_tool_request(&rows, &workspaces)
        .err()
        .ok_or_else(IpcError::internal)?;
    let repair = exact_argument_repair_error(&rows, &catalogue, &base);
    assert!(repair.message.contains("row_length_mismatch"));
    assert!(repair.message.contains("$.rows[0]"));
    assert!(repair.message.contains("receivedCellCount\":3"));
    assert!(repair.message.contains("$.rows[1]"));
    assert!(repair.message.contains("receivedCellCount\":1"));
    assert!(repair.message.contains("$.rows[2][1]"));
    Ok(())
}

#[test]
fn directory_creation_requires_a_writable_workspace() -> Result<(), IpcError> {
    let mut workspaces = vec![ChatToolWorkspace {
        alias: "primary".to_owned(),
        display_name: "zeta-dev".to_owned(),
        grant_id: WorkspaceGrantId("grant".to_owned()),
        permission_level: WorkspacePermissionLevel::ReadAndWrite,
    }];
    let call = ParsedToolCall {
        call_id: "call-directory".to_owned(),
        recipient: "filesystem.create_directory".to_owned(),
        name: "filesystem.create_directory".to_owned(),
        arguments: BTreeMap::from([
            (
                "path".to_owned(),
                serde_json::json!("outputs/reconstructed_tables/monthly cashflow"),
            ),
            ("workspace".to_owned(), serde_json::json!("primary")),
        ]),
    };
    assert_eq!(
        validated_chat_tool_request(&call, &workspaces)?.name,
        ToolName::FilesystemCreateDirectory
    );
    workspaces[0].permission_level = WorkspacePermissionLevel::ReadOnly;
    let Err(error) = validated_chat_tool_request(&call, &workspaces) else {
        return Err(IpcError::internal());
    };
    assert_eq!(error.code, ipc_types::IpcErrorCode::PermissionDenied);
    Ok(())
}

#[test]
fn forecastability_is_a_closed_tool_classification() {
    assert!(is_forecastable_chat_tool("computation.run"));
    assert!(is_forecastable_chat_tool("computation.run_project"));
    assert!(is_forecastable_chat_tool("filesystem.read_text"));
    assert!(!is_forecastable_chat_tool("orchestration.submit_plan"));
    assert!(!is_forecastable_chat_tool("orchestration.complete_stage"));
    assert!(!is_forecastable_chat_tool("orchestration.refine_stage"));
    assert!(!is_forecastable_chat_tool("orchestration.future_control"));
    assert!(!is_forecastable_chat_tool("computation.not_registered"));
}

#[cfg(debug_assertions)]
#[test]
fn x402_tool_accepts_discovery_and_rejects_urls_outside_the_configured_scope()
-> Result<(), IpcError> {
    let call = model_runtime::ParsedToolCall {
        call_id: "flowzed-1".to_owned(),
        recipient: "x402.request".to_owned(),
        name: "x402.request".to_owned(),
        arguments: BTreeMap::from([
            ("operation".to_owned(), serde_json::json!("discover")),
            (
                "reason".to_owned(),
                serde_json::json!("Discover the Atelier purchase contract"),
            ),
            (
                "url".to_owned(),
                serde_json::json!("http://127.0.0.1:5001/test-project/us-central1/api/x402/offers"),
            ),
        ]),
    };
    assert_eq!(
        validated_chat_tool_request(&call, &[])?.name,
        ToolName::X402Request
    );

    let mut changed = call;
    changed.arguments.insert(
        "url".to_owned(),
        serde_json::json!("http://127.0.0.1:5002/test-project/us-central1/api/x402/offers"),
    );
    assert!(validated_chat_tool_request(&changed, &[]).is_err());
    Ok(())
}

#[cfg(debug_assertions)]
#[test]
fn x402_legacy_request_shape_receives_the_host_field_correction() -> Result<(), IpcError> {
    let call = model_runtime::ParsedToolCall {
        call_id: "flowzed-legacy".to_owned(),
        recipient: "x402.request".to_owned(),
        name: "x402.request".to_owned(),
        arguments: BTreeMap::from([
            ("operation".to_owned(), serde_json::json!("purchase")),
            ("reason".to_owned(), serde_json::json!("Test purchase")),
            (
                "request".to_owned(),
                serde_json::json!({"requestId": "model-value"}),
            ),
            (
                "url".to_owned(),
                serde_json::json!("http://127.0.0.1:5001/test-project/us-central1/api/x402/offers"),
            ),
        ]),
    };
    let Err(error) = validated_chat_tool_request(&call, &[]) else {
        return Err(IpcError::internal());
    };
    assert!(error.message.contains("obsolete Flowzed quote contract"));
    assert!(error.message.contains("resourceRef"));
    assert!(error.message.contains("hostGeneratedFields"));
    Ok(())
}

#[cfg(debug_assertions)]
#[test]
fn x402_missing_resource_reference_repairs_the_discovered_branch_only() -> Result<(), IpcError> {
    let call = model_runtime::ParsedToolCall {
        call_id: "x402-missing-reference".to_owned(),
        recipient: "x402.request".to_owned(),
        name: "x402.request".to_owned(),
        arguments: BTreeMap::from([
            ("operation".to_owned(), serde_json::json!("purchase")),
            ("reason".to_owned(), serde_json::json!("Test purchase")),
            ("input".to_owned(), serde_json::json!({"selection": "blue"})),
        ]),
    };
    let catalogue = chat_tool_definitions(&[])?;
    let base = validated_chat_tool_request(&call, &[])
        .err()
        .ok_or_else(IpcError::internal)?;
    let repair = exact_argument_repair_error(&call, &catalogue, &base);
    assert!(repair.message.contains("`$.resourceRef`"));
    assert!(repair.message.contains("Preserve `$.input`"));
    assert!(repair.message.contains("Do not add `$.url`"));
    Ok(())
}

#[test]
fn nested_schema_repair_names_exact_path_enum_and_complete_operation_schema() -> Result<(), IpcError>
{
    let workspaces = vec![ChatToolWorkspace {
        alias: "primary".to_owned(),
        display_name: "workspace".to_owned(),
        grant_id: WorkspaceGrantId("grant".to_owned()),
        permission_level: WorkspacePermissionLevel::ReadAndWrite,
    }];
    let catalogue = chat_tool_definitions(&workspaces)?;
    let call = ParsedToolCall {
        call_id: "invalid-nested-enum".to_owned(),
        recipient: "filesystem.collect_evidence".to_owned(),
        name: "filesystem.collect_evidence".to_owned(),
        arguments: BTreeMap::from([
            (
                "items".to_owned(),
                serde_json::json!([{
                    "id": "source",
                    "requirement": "Load the requested source",
                    "nameContains": "source",
                    "operation": "guess_representative",
                    "scope": "descendants",
                    "path": ".",
                    "extensions": ["csv"],
                    "maxDepth": 4
                }]),
            ),
            ("workspace".to_owned(), serde_json::json!("primary")),
        ]),
    };
    let base = validated_chat_tool_request(&call, &workspaces)
        .err()
        .ok_or_else(IpcError::internal)?;
    let repaired = exact_argument_repair_error(&call, &catalogue, &base);
    assert!(repaired.message.contains("`filesystem.collect_evidence`"));
    assert!(repaired.message.contains("`$.items[0].operation`"));
    assert!(
        repaired
            .message
            .contains("expected one of [\"find_matches\"]")
    );
    assert!(
        repaired
            .message
            .contains("every other defect reported in the validation detail")
    );
    assert!(!repaired.message.contains("Complete relevant schema:"));
    assert!(!repaired.message.contains("\"additionalProperties\":false"));

    let misplaced_path = ParsedToolCall {
        call_id: "misplaced-path".to_owned(),
        recipient: "filesystem.collect_evidence".to_owned(),
        name: "filesystem.collect_evidence".to_owned(),
        arguments: BTreeMap::from([
            (
                "items".to_owned(),
                serde_json::json!([{
                    "id": "source",
                    "requirement": "Locate the source",
                    "operation": "find_matches",
                    "path": ".",
                    "extensions": ["png"]
                }]),
            ),
            ("path".to_owned(), serde_json::json!(".")),
            ("workspace".to_owned(), serde_json::json!("primary")),
        ]),
    };
    let base = validated_chat_tool_request(&misplaced_path, &workspaces)
        .err()
        .ok_or_else(IpcError::internal)?;
    let repaired = exact_argument_repair_error(&misplaced_path, &catalogue, &base);
    assert!(repaired.message.contains("`$.path`"));
    assert!(repaired.message.contains("permits only: items, workspace"));
    assert!(!repaired.message.contains("Complete relevant schema:"));
    Ok(())
}
