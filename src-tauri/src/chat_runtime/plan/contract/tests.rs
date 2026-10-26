use super::*;

fn semantic_arguments() -> BTreeMap<String, serde_json::Value> {
    BTreeMap::from([
        ("assumptions".to_owned(), serde_json::json!([])),
        (
            "constraints".to_owned(),
            serde_json::json!(["Preserve source values"]),
        ),
        (
            "successCriteria".to_owned(),
            serde_json::json!(["The requested artifact is verified"]),
        ),
        (
            "steps".to_owned(),
            serde_json::json!([
                {
                    "actionClass": "read",
                    "dependsOn": [],
                    "doneWhen": "The source content is available",
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
        ("target".to_owned(), serde_json::json!("result/output.csv")),
    ])
}

#[test]
fn semantic_contract_accepts_optional_preferences_and_satisfiable_dependencies()
-> Result<(), IpcError> {
    let available = BTreeSet::from(["filesystem.inspect_image", "filesystem.write_text"]);
    let contract = SemanticPlanContract::parse(
        &semantic_arguments(),
        &available,
        16,
        PlanContractVersion::SemanticExecutionV2,
    )?;
    assert_eq!(contract.steps.len(), 2);
    assert_eq!(
        contract.steps[0].preferred_tools,
        ["filesystem.inspect_image"]
    );
    assert!(contract.steps[1].preferred_tools.is_empty());
    assert_eq!(contract.steps[1].depends_on, ["load-source"]);
    Ok(())
}

#[test]
fn semantic_contract_orders_forward_acyclic_dependencies() -> Result<(), IpcError> {
    let available = BTreeSet::from(["filesystem.inspect_image"]);
    let mut arguments = semantic_arguments();
    let Some(dependencies) = arguments
        .get_mut("steps")
        .and_then(|steps| steps.pointer_mut("/0/dependsOn"))
    else {
        return Err(IpcError::internal());
    };
    *dependencies = serde_json::json!(["write-output"]);
    *arguments
        .get_mut("steps")
        .and_then(|steps| steps.pointer_mut("/1/dependsOn"))
        .ok_or_else(IpcError::internal)? = serde_json::json!([]);
    let contract = SemanticPlanContract::parse(
        &arguments,
        &available,
        16,
        PlanContractVersion::SemanticExecutionV2,
    )?;
    assert_eq!(contract.steps[0].id, "write-output");
    assert_eq!(contract.steps[1].id, "load-source");
    Ok(())
}

#[test]
fn semantic_contract_rejects_cycles_and_unknown_dependencies() -> Result<(), IpcError> {
    let available = BTreeSet::from(["filesystem.inspect_image"]);
    let mut cyclic = semantic_arguments();
    *cyclic
        .get_mut("steps")
        .and_then(|steps| steps.pointer_mut("/0/dependsOn"))
        .ok_or_else(IpcError::internal)? = serde_json::json!(["write-output"]);
    let Err(error) = SemanticPlanContract::parse(
        &cyclic,
        &available,
        16,
        PlanContractVersion::SemanticExecutionV2,
    ) else {
        return Err(IpcError::internal());
    };
    assert!(error.message.contains("cycle"));

    let mut unknown = semantic_arguments();
    *unknown
        .get_mut("steps")
        .and_then(|steps| steps.pointer_mut("/1/dependsOn"))
        .ok_or_else(IpcError::internal)? = serde_json::json!(["missing"]);
    let Err(error) = SemanticPlanContract::parse(
        &unknown,
        &available,
        16,
        PlanContractVersion::SemanticExecutionV2,
    ) else {
        return Err(IpcError::internal());
    };
    assert!(error.message.contains("unknown step `missing`"));
    Ok(())
}

#[test]
fn semantic_contract_rejects_preference_that_hides_a_riskier_action() -> Result<(), IpcError> {
    let available = BTreeSet::from(["filesystem.inspect_image", "filesystem.write_text"]);
    let mut arguments = semantic_arguments();
    *arguments
        .get_mut("steps")
        .and_then(|steps| steps.pointer_mut("/0/preferredTools/0"))
        .ok_or_else(IpcError::internal)? = serde_json::json!("filesystem.write_text");
    let Err(error) = SemanticPlanContract::parse(
        &arguments,
        &available,
        16,
        PlanContractVersion::SemanticExecutionV2,
    ) else {
        return Err(IpcError::internal());
    };
    assert!(error.message.contains("actionClass `read`"));
    assert!(error.message.contains("filesystem.write_text"));
    Ok(())
}

#[test]
fn contract_version_keeps_legacy_calls_interpretable() -> Result<(), IpcError> {
    let legacy = ParsedToolCall {
        call_id: "legacy".to_owned(),
        recipient: "orchestration.submit_plan".to_owned(),
        name: "orchestration.submit_plan".to_owned(),
        arguments: BTreeMap::new(),
    };
    assert_eq!(
        PlanContractVersion::from_call(&legacy)?,
        PlanContractVersion::LegacyForecastV1
    );
    let mut semantic_v1 = semantic_arguments();
    semantic_v1.insert(
        "contractVersion".to_owned(),
        serde_json::json!(LEGACY_SEMANTIC_PLAN_CONTRACT_VERSION),
    );
    *semantic_v1
        .get_mut("steps")
        .and_then(|steps| steps.pointer_mut("/1/actionClass"))
        .ok_or_else(IpcError::internal)? = serde_json::json!("local_write");
    let semantic_v1_call = ParsedToolCall {
        arguments: semantic_v1.clone(),
        ..legacy.clone()
    };
    assert_eq!(
        PlanContractVersion::from_call(&semantic_v1_call)?,
        PlanContractVersion::SemanticExecutionV1
    );
    SemanticPlanContract::parse(
        &semantic_v1,
        &BTreeSet::from(["filesystem.inspect_image", "filesystem.write_text"]),
        16,
        PlanContractVersion::SemanticExecutionV1,
    )?;

    let error = SemanticPlanContract::parse(
        &semantic_arguments(),
        &BTreeSet::from(["filesystem.inspect_image", "filesystem.write_text"]),
        16,
        PlanContractVersion::SemanticExecutionV1,
    )
    .err()
    .ok_or_else(IpcError::internal)?;
    assert!(error.message.contains("unknown actionClass"));

    let mut semantic_v2 = semantic_arguments();
    semantic_v2.insert(
        "contractVersion".to_owned(),
        serde_json::json!(ROLELESS_SEMANTIC_PLAN_CONTRACT_VERSION),
    );
    let semantic_v2_call = ParsedToolCall {
        arguments: semantic_v2.clone(),
        ..legacy
    };
    assert_eq!(
        PlanContractVersion::from_call(&semantic_v2_call)?,
        PlanContractVersion::SemanticExecutionV2
    );
    *semantic_v2
        .get_mut("steps")
        .and_then(|steps| steps.pointer_mut("/1/actionClass"))
        .ok_or_else(IpcError::internal)? = serde_json::json!("local_write");
    let error = SemanticPlanContract::parse(
        &semantic_v2,
        &BTreeSet::from(["filesystem.inspect_image", "filesystem.write_text"]),
        16,
        PlanContractVersion::SemanticExecutionV2,
    )
    .err()
    .ok_or_else(IpcError::internal)?;
    assert!(error.message.contains("unknown actionClass"));
    Ok(())
}

#[test]
fn semantic_v3_requires_closed_unique_typed_obligations() -> Result<(), IpcError> {
    let mut arguments = semantic_arguments();
    arguments.insert(
            "steps".to_owned(),
            serde_json::json!([{
                "actionClass":"read","dependsOn":[],"doneWhen":"Content loaded","id":"read","objective":"Load content",
                "obligations":[{"id":"source-table","roleKind":"source","evidenceClass":"semantic_content","postcondition":"content_loaded"}]
            }]),
        );
    let contract = SemanticPlanContract::parse(
        &arguments,
        &BTreeSet::new(),
        16,
        PlanContractVersion::SemanticExecutionV3,
    )?;
    assert_eq!(contract.steps[0].obligations[0].id, "source-table");

    *arguments
        .get_mut("steps")
        .and_then(|steps| steps.pointer_mut("/0/obligations/0/roleKind"))
        .ok_or_else(IpcError::internal)? = serde_json::json!("arbitrary-role");
    let error = SemanticPlanContract::parse(
        &arguments,
        &BTreeSet::new(),
        16,
        PlanContractVersion::SemanticExecutionV3,
    )
    .err()
    .ok_or_else(IpcError::internal)?;
    assert!(error.message.contains("unknown obligation roleKind"));
    Ok(())
}

#[test]
fn semantic_v4_derives_internal_obligations_from_observable_kinds() -> Result<(), IpcError> {
    let mut arguments = semantic_arguments();
    arguments.insert(
            "steps".to_owned(),
            serde_json::json!([
                {
                    "actionClass":"read","dependsOn":[],"doneWhen":"Image loaded","id":"source","objective":"Load source",
                    "obligations":[{"id":"source-image","kind":"image_content_loaded","roleKind":"source"}]
                },
                {
                    "actionClass":"workspace_write","dependsOn":["source"],"doneWhen":"File verified","id":"write","objective":"Write output",
                    "obligations":[
                        {"id":"grounded-source","kind":"image_content_loaded","roleKind":"source"},
                        {"id":"written-file","kind":"file_write_verified"}
                    ]
                }
            ]),
        );
    let contract = SemanticPlanContract::parse(
        &arguments,
        &BTreeSet::new(),
        16,
        PlanContractVersion::SemanticExecutionV4,
    )?;
    assert_eq!(contract.steps.len(), 2);
    let written = &contract.steps[1].obligations[1];
    assert_eq!(written.role_kind, PlanEvidenceRoleKind::Target);
    assert_eq!(written.evidence_class, PlanEvidenceClass::IdentityLocation);
    assert_eq!(
        written.postcondition,
        PlanEvidencePostcondition::FileWriteVerified
    );
    Ok(())
}

#[test]
fn semantic_v4_aggregates_tuple_mismatches_with_exact_replacements() -> Result<(), IpcError> {
    let mut arguments = semantic_arguments();
    arguments.insert(
            "steps".to_owned(),
            serde_json::json!([
                {
                    "actionClass":"read","dependsOn":[],"doneWhen":"Loaded","id":"source","objective":"Load source",
                    "obligations":[{"id":"source-table","roleKind":"source","evidenceClass":"semantic_content","postcondition":"content_loaded"}]
                },
                {
                    "actionClass":"compute","dependsOn":["source"],"doneWhen":"Computed","id":"compute","objective":"Compute table",
                    "obligations":[{"id":"computed","kind":"identity_located","roleKind":"target"}]
                },
                {
                    "actionClass":"workspace_write","dependsOn":["compute"],"doneWhen":"Written","id":"write","objective":"Write file",
                    "obligations":[{"id":"written","kind":"computation_completed"}]
                }
            ]),
        );
    let error = SemanticPlanContract::parse(
        &arguments,
        &BTreeSet::new(),
        16,
        PlanContractVersion::SemanticExecutionV4,
    )
    .err()
    .ok_or_else(IpcError::internal)?;
    let defects = plan_repair_defects(&error).ok_or_else(IpcError::internal)?;
    assert!(defects.len() >= 3);
    assert!(error.message.contains("/steps/0/obligations/0"));
    assert!(error.message.contains("/steps/1/obligations"));
    assert!(error.message.contains("computation_completed"));
    assert!(error.message.contains("file_write_verified"));
    assert!(error.message.contains("Preserve unrelated plan content"));

    let v5_error = SemanticPlanContract::parse(
        &arguments,
        &BTreeSet::new(),
        16,
        PlanContractVersion::SemanticExecutionV5,
    )
    .err()
    .ok_or_else(IpcError::internal)?;
    assert!(
        v5_error
            .message
            .starts_with("semantic_execution_v5 plan defects: ")
    );
    let v5_defects = plan_repair_defects(&v5_error).ok_or_else(IpcError::internal)?;
    assert!(
        v5_defects
            .iter()
            .all(|(identity, _)| identity.starts_with("semantic_execution_v5:"))
    );
    Ok(())
}

#[test]
fn semantic_v6_aggregates_native_array_and_suggested_tool_repairs() -> Result<(), IpcError> {
    let mut arguments = BTreeMap::from([
        ("assumptions".to_owned(), serde_json::json!([])),
        (
            "constraints".to_owned(),
            serde_json::json!("Preserve source values"),
        ),
        (
            "steps".to_owned(),
            serde_json::json!([
                {
                    "actionClass":"read","dependsOn":[],"doneWhen":"Located",
                    "id":"locate","objective":"Locate the source",
                    "suggestedTools":["filesystem.find_paths"]
                },
                {
                    "actionClass":"workspace_write","dependsOn":["locate"],"doneWhen":"Written",
                    "id":"write","objective":"Write the result",
                    "suggestedTools":["filesystem.unavailable"]
                }
            ]),
        ),
        (
            "successCriteria".to_owned(),
            serde_json::json!(["The file exists"]),
        ),
        ("target".to_owned(), serde_json::json!("result.csv")),
    ]);
    let available = BTreeSet::from(["filesystem.find_paths", "filesystem.write_text"]);
    let error = SemanticPlanContract::parse(
        &arguments,
        &available,
        16,
        PlanContractVersion::SemanticExecutionV6,
    )
    .err()
    .ok_or_else(IpcError::internal)?;
    assert!(error.message.contains("/constraints"));
    assert!(error.message.contains("/steps/1/suggestedTools/0"));
    assert!(error.message.contains("neverStringify"));
    assert!(error.message.contains("filesystem.write_text"));
    assert!(error.message.contains("availableTools"));
    assert!(!error.message.contains("orMoveToolToStageWithActionClass"));
    assert_eq!(
        plan_repair_defects(&error)
            .ok_or_else(IpcError::internal)?
            .len(),
        2
    );

    arguments.insert(
        "constraints".to_owned(),
        serde_json::json!(["Preserve source values"]),
    );
    arguments.insert(
        "steps".to_owned(),
        serde_json::json!("[{\"id\":\"locate\",\"actionClass\":\"read\",\"dependsOn\":[]}]"),
    );
    let stringified_error = SemanticPlanContract::parse(
        &arguments,
        &available,
        16,
        PlanContractVersion::SemanticExecutionV6,
    )
    .err()
    .ok_or_else(IpcError::internal)?;
    assert!(stringified_error.message.contains("/steps"));
    assert!(stringified_error.message.contains("native JSON arrays"));
    assert!(!stringified_error.message.contains("one to sixteen stages"));
    Ok(())
}
