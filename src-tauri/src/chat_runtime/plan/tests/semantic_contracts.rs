use super::*;

#[test]
fn forecast_categories_cover_generic_text_image_table_compute_and_mixed_routes() {
    assert_eq!(
        operation_category("filesystem.read_text"),
        Some("workspace_read")
    );
    assert_eq!(
        operation_category("filesystem.inspect_image"),
        Some("workspace_read")
    );
    assert_eq!(
        operation_category("filesystem.write_delimited_table"),
        Some("workspace_write")
    );
    assert_eq!(
        operation_category("computation.run_project"),
        Some("computation")
    );
    assert_eq!(
        operation_category("filesystem.collect_evidence"),
        Some("workspace_read")
    );
    assert_eq!(operation_category("web.search"), Some("web"));
    assert_eq!(operation_category(PLAN_TOOL), Some("orchestration"));
    assert_eq!(operation_category(FORECAST_TOOL), Some("orchestration"));
}

#[test]
fn semantic_step_without_preferences_advances_from_its_action_class() -> Result<(), IpcError> {
    let all = vec![
        tool("filesystem.inspect_image"),
        tool("filesystem.write_text"),
        tool(PLAN_TOOL),
    ];
    let mut policy = PlanPolicy::default();
    policy.accept(&semantic_plan_call("initial"), &all, 8)?;

    let inspect = operation("inspect", "filesystem.inspect_image");
    policy.record_results(
        std::slice::from_ref(&inspect),
        &[Ok(completed_operation(
            "inspect",
            ToolName::FilesystemInspectImage,
        ))],
    );
    assert!(policy.forecast_steps[0].complete());
    assert!(policy.forecast_steps[1].unfinished());

    let write = operation("write", "filesystem.write_text");
    policy.record_results(
        std::slice::from_ref(&write),
        &[Ok(completed_operation(
            "write",
            ToolName::FilesystemWriteText,
        ))],
    );
    assert!(policy.planned_outcome_satisfied());
    assert!(policy.forecast_complete());
    Ok(())
}

#[test]
fn semantic_preference_is_non_exhaustive_and_an_equivalent_tool_advances_the_step()
-> Result<(), IpcError> {
    let all = [
        tool(PLAN_TOOL),
        tool(FORECAST_TOOL),
        tool("filesystem.inspect_image"),
        tool("filesystem.read_text"),
        tool("filesystem.write_text"),
    ];
    let mut policy = PlanPolicy::default();
    policy.accept(&semantic_plan_call("initial"), &all, 10)?;
    policy.accept_forecast(&semantic_forecast_call("initial"), &all)?;

    let alternative = operation("text-evidence", "filesystem.read_text");
    assert_eq!(
        policy.diagnostic_match(&alternative).2,
        ForecastDiagnosticMatch::Matched
    );
    policy.validate_calls(std::slice::from_ref(&alternative))?;
    let exchanges = [Ok(completed_operation(
        "text-evidence",
        ToolName::FilesystemReadText,
    ))];
    let records = policy.admission_records(std::slice::from_ref(&alternative), &exchanges);
    assert_eq!(records[0].permission, OperationPermissionOutcome::Admitted);
    assert_eq!(
        records[0].relationship,
        OperationStepRelationship::AdaptiveCurrent
    );
    assert_eq!(records[0].progress, OperationProgressContribution::Advanced);
    policy.record_results(std::slice::from_ref(&alternative), &exchanges);

    assert!(policy.forecast_steps[0].complete());
    assert!(!policy.forecast_steps[1].complete());
    assert_eq!(policy.unforecasted_operations, 0);
    Ok(())
}

#[test]
fn semantic_admission_rejects_dependency_skips_and_unrelated_scope_without_revoking_tools()
-> Result<(), IpcError> {
    let all = [
        tool(PLAN_TOOL),
        tool(FORECAST_TOOL),
        tool("filesystem.inspect_image"),
        tool("filesystem.write_text"),
        tool("computation.run"),
    ];
    let mut policy = PlanPolicy::default();
    policy.accept(&semantic_plan_call("initial"), &all, 10)?;
    policy.accept_forecast(&semantic_forecast_call("initial"), &all)?;

    let future_write = operation("early-write", "filesystem.write_text");
    let Err(error) = policy.validate_calls(std::slice::from_ref(&future_write)) else {
        return Err(IpcError::internal());
    };
    assert_eq!(error.code, IpcErrorCode::PlanMismatch);
    assert_eq!(
        policy.operation_relationship(&future_write),
        OperationStepRelationship::FutureStep
    );

    let unrelated = operation("unrelated", "computation.run");
    let Err(error) = policy.validate_calls(std::slice::from_ref(&unrelated)) else {
        return Err(IpcError::internal());
    };
    assert_eq!(error.code, IpcErrorCode::ScopeCorrection);
    let projected = policy
        .tools(&all)
        .into_iter()
        .map(|tool| tool.name)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        projected,
        BTreeSet::from([
            PLAN_TOOL.to_owned(),
            FORECAST_TOOL.to_owned(),
            "filesystem.inspect_image".to_owned(),
        ])
    );
    Ok(())
}

#[test]
fn semantic_ready_frontier_batches_independent_work_but_withholds_downstream_credit()
-> Result<(), IpcError> {
    let all = [
        tool(PLAN_TOOL),
        tool("filesystem.inspect_image"),
        tool("filesystem.read_text"),
        tool("filesystem.write_text"),
    ];
    let mut plan = semantic_plan_call("initial");
    plan.arguments.insert(
        "steps".to_owned(),
        serde_json::json!([
            {
                "actionClass":"read", "dependsOn":[],
                "doneWhen":"Image content loaded", "id":"image",
                "objective":"Load image", "preferredTools":["filesystem.inspect_image"],
                "requiredEvidence":["semantic_content"]
            },
            {
                "actionClass":"read", "dependsOn":[],
                "doneWhen":"Convention content loaded", "id":"convention",
                "objective":"Load convention", "preferredTools":["filesystem.read_text"],
                "requiredEvidence":["semantic_content"]
            },
            {
                "actionClass":"workspace_write", "dependsOn":["image","convention"],
                "doneWhen":"Output verified", "id":"write",
                "objective":"Write output", "preferredTools":["filesystem.write_text"],
                "requiredEvidence":[]
            }
        ]),
    );
    let mut policy = PlanPolicy::default();
    policy.accept(&plan, &all, 10)?;

    let image = operation("image-call", "filesystem.inspect_image");
    let convention = operation("convention-call", "filesystem.read_text");
    policy.validate_calls(&[image.clone(), convention.clone()])?;
    assert_eq!(
        policy.operation_relationship(&image),
        OperationStepRelationship::ReadyIndependent
    );
    let write = operation("early-write", "filesystem.write_text");
    let Err(error) = policy.validate_calls(std::slice::from_ref(&write)) else {
        return Err(IpcError::internal());
    };
    assert_eq!(error.code, IpcErrorCode::PlanMismatch);

    policy.record_results(
        &[image, convention],
        &[
            Ok(completed_operation(
                "image-call",
                ToolName::FilesystemInspectImage,
            )),
            Ok(completed_operation(
                "convention-call",
                ToolName::FilesystemReadText,
            )),
        ],
    );
    assert!(
        policy
            .forecast_steps
            .iter()
            .find(|step| step.id == "image")
            .is_some_and(ForecastStep::complete)
    );
    assert!(
        policy
            .forecast_steps
            .iter()
            .find(|step| step.id == "convention")
            .is_some_and(ForecastStep::complete)
    );
    assert!(
        policy
            .forecast_steps
            .iter()
            .find(|step| step.id == "write")
            .is_some_and(ForecastStep::unfinished)
    );
    assert_eq!(
        policy.current_step_unfinished_tools(),
        vec!["filesystem.write_text"]
    );
    Ok(())
}

#[test]
fn semantic_refinement_returns_to_its_suspended_parent_step() -> Result<(), IpcError> {
    let all = [
        tool(PLAN_TOOL),
        tool(FORECAST_TOOL),
        tool("filesystem.find_paths"),
        tool("filesystem.inspect_image"),
        tool("filesystem.write_text"),
    ];
    let mut policy = PlanPolicy::default();
    policy.accept(&semantic_plan_call("initial"), &all, 10)?;
    policy.accept_forecast(&semantic_forecast_call("initial"), &all)?;

    let discovery = operation("discover", "filesystem.find_paths");
    policy.validate_calls(std::slice::from_ref(&discovery))?;
    policy.record_results(
        std::slice::from_ref(&discovery),
        &[Ok(completed_find_paths_operation("discover", &[(1, true)]))],
    );
    assert!(policy.refinement_active());
    assert!(!policy.forecast_steps[0].complete());

    let mut revision = semantic_plan_call("revise");
    revision
        .arguments
        .get_mut("steps")
        .and_then(serde_json::Value::as_array_mut)
        .and_then(|steps| steps.first_mut())
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(IpcError::internal)?
        .insert(
            "doneWhen".to_owned(),
            serde_json::json!("The exact source content is loaded"),
        );
    policy.accept(&revision, &all, 10)?;
    policy.accept_forecast(&semantic_forecast_call("initial"), &all)?;
    assert_eq!(policy.revision(), 2);
    assert!(policy.refinement_active());
    assert_eq!(
        policy
            .unresolved_evidence_call_ids
            .get("load-source")
            .map(String::as_str),
        Some("discover")
    );

    let inspect = operation("inspect", "filesystem.inspect_image");
    policy.validate_calls(std::slice::from_ref(&inspect))?;
    policy.record_results(
        std::slice::from_ref(&inspect),
        &[Ok(completed_operation(
            "inspect",
            ToolName::FilesystemInspectImage,
        ))],
    );
    assert!(!policy.refinement_active());
    assert!(
        policy
            .forecast_steps
            .iter()
            .find(|step| step.id == "load-source")
            .is_some_and(ForecastStep::complete)
    );
    let write = operation("write", "filesystem.write_text");
    assert_eq!(
        policy.operation_relationship(&write),
        OperationStepRelationship::PreferredCurrent
    );
    assert!(
        policy
            .current_step_relevant_tools(&all)
            .contains(&"filesystem.write_text")
    );
    policy.validate_calls(std::slice::from_ref(&write))?;
    Ok(())
}

#[test]
fn semantic_path_discovery_is_same_step_refinement_until_required_evidence_is_loaded()
-> Result<(), IpcError> {
    let all = [
        tool(PLAN_TOOL),
        tool("filesystem.find_paths"),
        tool("filesystem.inspect_image"),
        tool("filesystem.write_text"),
    ];
    let mut policy = PlanPolicy::default();
    policy.accept(&semantic_plan_call("initial"), &all, 10)?;

    let discovery = operation("discover", "filesystem.find_paths");
    policy.record_results(
        std::slice::from_ref(&discovery),
        &[Ok(completed_find_paths_operation("discover", &[(1, true)]))],
    );
    assert!(!policy.forecast_steps[0].complete());
    assert!(policy.forecast_steps[0].has_completed_work());
    assert!(!policy.deviation_pending());

    let load = operation("load", "filesystem.inspect_image");
    policy.record_results(
        std::slice::from_ref(&load),
        &[Ok(completed_operation(
            "load",
            ToolName::FilesystemInspectImage,
        ))],
    );
    assert!(policy.forecast_steps[0].complete());
    assert_eq!(policy.unforecasted_operations, 0);
    Ok(())
}

#[test]
fn semantic_revision_preserves_reusable_progress_and_is_idempotent() -> Result<(), IpcError> {
    let all = [
        tool(PLAN_TOOL),
        tool("filesystem.inspect_image"),
        tool("filesystem.read_text"),
        tool("filesystem.write_text"),
    ];
    let mut policy = PlanPolicy::default();
    policy.accept(&semantic_plan_call("initial"), &all, 10)?;
    let load = operation("load", "filesystem.inspect_image");
    policy.record_results(
        std::slice::from_ref(&load),
        &[Ok(completed_operation(
            "load",
            ToolName::FilesystemInspectImage,
        ))],
    );

    let mut revision = semantic_plan_call("revise");
    revision
        .arguments
        .insert("outcome".to_owned(), serde_json::json!("modify_file"));
    revision
        .arguments
        .insert("target".to_owned(), serde_json::json!("result/revised.csv"));
    let accepted = policy.accept(&revision, &all, 8)?;
    assert_eq!(accepted["revision"], 2);
    assert!(policy.forecast_steps[0].complete());
    assert!(!policy.forecast_steps[1].complete());
    assert_eq!(policy.operations_used(), 1);

    let mut unchanged_route_hint = revision;
    *unchanged_route_hint
        .arguments
        .get_mut("steps")
        .and_then(|steps| steps.pointer_mut("/0/preferredTools/0"))
        .ok_or_else(IpcError::internal)? = serde_json::json!("filesystem.read_text");
    let unchanged = policy.accept(&unchanged_route_hint, &all, 8)?;
    assert_eq!(unchanged["planUnchanged"], true);
    assert_eq!(policy.revision, 2);
    assert_eq!(policy.revisions_used, 1);
    Ok(())
}

#[test]
fn semantic_outcome_rejects_hidden_external_authority() -> Result<(), IpcError> {
    let all = [tool(PLAN_TOOL), tool("filesystem.inspect_image")];
    let mut plan = semantic_plan_call("initial");
    plan.arguments
        .insert("outcome".to_owned(), serde_json::json!("other"));
    plan.arguments.insert(
        "steps".to_owned(),
        serde_json::json!([{
            "actionClass": "external_write",
            "dependsOn": [],
            "doneWhen": "The external effect is confirmed",
            "id": "publish",
            "objective": "Publish the result",
            "requiredEvidence": []
        }]),
    );
    let mut policy = PlanPolicy::default();
    let Err(error) = policy.accept(&plan, &all, 10) else {
        return Err(IpcError::internal());
    };
    assert!(error.message.contains("external_write"));
    assert!(error.message.contains("expanded authority"));
    Ok(())
}

#[test]
fn semantic_file_outcome_requires_an_outcome_bearing_write_receipt() -> Result<(), IpcError> {
    let all = [
        tool(PLAN_TOOL),
        tool("filesystem.inspect_image"),
        tool("filesystem.create_directory"),
        tool("filesystem.write_text"),
    ];
    let mut policy = PlanPolicy::default();
    policy.accept(&semantic_plan_call("initial"), &all, 10)?;
    let load = operation("load", "filesystem.inspect_image");
    policy.record_results(
        std::slice::from_ref(&load),
        &[Ok(completed_operation(
            "load",
            ToolName::FilesystemInspectImage,
        ))],
    );

    let directory = operation("directory", "filesystem.create_directory");
    policy.record_results(
        std::slice::from_ref(&directory),
        &[Ok(completed_operation(
            "directory",
            ToolName::FilesystemCreateDirectory,
        ))],
    );
    assert!(!policy.forecast_steps[1].complete());
    assert!(!policy.planned_outcome_satisfied());

    let write = operation("write", "filesystem.write_text");
    policy.record_results(
        std::slice::from_ref(&write),
        &[Ok(completed_operation(
            "write",
            ToolName::FilesystemWriteText,
        ))],
    );
    assert!(policy.forecast_steps[1].complete());
    assert!(policy.planned_outcome_satisfied());
    Ok(())
}

#[test]
fn accepted_plan_revisions_cannot_change_contract_version() -> Result<(), IpcError> {
    let all = vec![
        tool("filesystem.inspect_image"),
        tool("filesystem.write_text"),
        tool(PLAN_TOOL),
    ];
    let mut policy = PlanPolicy::default();
    policy.accept(&semantic_plan_call("initial"), &all, 8)?;
    let Err(error) = policy.accept(&plan_call("revise", &["filesystem.write_text"]), &all, 8)
    else {
        return Err(IpcError::internal());
    };
    assert!(
        error
            .message
            .contains("preserve the accepted contractVersion")
    );
    Ok(())
}

#[test]
fn bounded_artifact_route_requires_ordered_evidence_then_transformation() -> Result<(), IpcError> {
    let all = [
        tool(PLAN_TOOL),
        tool("filesystem.collect_evidence"),
        tool("orchestration.transform_artifact"),
    ];
    let mut ordered = plan_call("initial", &["orchestration.transform_artifact"]);
    ordered
        .arguments
        .insert("outcome".to_owned(), serde_json::json!("create_file"));
    ordered.arguments.insert(
        "steps".to_owned(),
        serde_json::json!([
            {
                "completionEvidence": "Source and convention are loaded",
                "expectedCalls": [{
                    "purpose":"Load exact source and convention",
                    "providesEvidence":[
                        {"role":"source","evidenceClass":"semantic_content"},
                        {"role":"convention","evidenceClass":"format_structure"}
                    ],
                    "tool":"filesystem.collect_evidence"
                }],
                "id": "evidence", "purpose": "Collect evidence",
                "requiredEvidence": ["Source image and convention manifest"]
            },
            {
                "completionEvidence": "The serialized artifact is post-write verified",
                "expectedCalls": [{"purpose":"Materialize the closed representation", "providesEvidence":[], "tool":"orchestration.transform_artifact"}],
                "id": "artifact", "purpose": "Transform and write",
                "requiredEvidence": ["Completed source and convention call IDs"]
            }
        ]),
    );
    let mut policy = PlanPolicy::default();
    let accepted = policy.accept(&ordered, &all, 8)?;
    assert_eq!(accepted["estimatedCalls"], 2);
    let evidence = typed_collector_operation("evidence", true);
    policy.record_results(
        std::slice::from_ref(&evidence),
        &[Ok(completed_operation(
            "evidence",
            ToolName::FilesystemCollectEvidence,
        ))],
    );
    let blocked_transform = artifact_operation("transform-blocked", "evidence", "evidence");
    policy.record_results(
        std::slice::from_ref(&blocked_transform),
        &[Ok(failed_operation(
            "transform-blocked",
            ToolName::OrchestrationTransformArtifact,
        ))],
    );
    let mut revision = ordered.clone();
    revision.call_id = "plan-revise".to_owned();
    revision
        .arguments
        .insert("mode".to_owned(), serde_json::json!("revise"));
    revision.arguments.insert(
        "steps".to_owned(),
        serde_json::json!([{
            "completionEvidence": "The serialized artifact is post-write verified",
            "expectedCalls": [{"purpose":"Materialize the closed representation", "providesEvidence":[], "tool":"orchestration.transform_artifact"}],
            "id": "artifact", "purpose": "Transform and write",
            "requiredEvidence": ["Previously completed typed source and convention evidence"]
        }]),
    );
    policy.accept(&revision, &all, 8)?;

    let wrong_binding = artifact_operation("transform-wrong", "discovery", "evidence");
    let Err(error) = policy.validate_calls(std::slice::from_ref(&wrong_binding)) else {
        return Err(IpcError::invalid_request(
            "identity or unknown source binding was accepted",
        ));
    };
    assert!(error.message.contains("exact call ID"));
    assert!(error.message.contains("semantic_content"));

    let transform = artifact_operation("transform", "evidence", "evidence");
    policy.validate_calls(std::slice::from_ref(&transform))?;
    policy.record_results(
        &[transform],
        &[Ok(completed_operation(
            "transform",
            ToolName::OrchestrationTransformArtifact,
        ))],
    );
    assert!(policy.planned_outcome_satisfied());

    let mut unordered = ordered;
    unordered.arguments.insert(
        "steps".to_owned(),
        serde_json::json!([{
            "completionEvidence": "Artifact exists",
            "expectedCalls": [{"purpose":"Transform", "tool":"orchestration.transform_artifact"}],
            "id": "artifact", "purpose": "Transform immediately",
            "requiredEvidence": ["Unbound evidence"]
        }]),
    );
    let Err(error) = PlanPolicy::default().accept(&unordered, &all, 8) else {
        return Err(IpcError::internal());
    };
    assert!(error.message.contains("typed evidence"));
    Ok(())
}
