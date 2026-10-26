use super::*;

#[test]
fn semantic_v6_treats_learned_assumptions_and_route_hints_as_unchanged() -> Result<(), IpcError> {
    let all = vec![
        tool("filesystem.inspect_image"),
        tool("filesystem.read_text"),
        tool("filesystem.write_text"),
        tool(PLAN_TOOL),
        tool(COMPLETE_STAGE_TOOL),
    ];
    let mut policy = PlanPolicy::default();
    policy.accept(&semantic_v6_plan_call("initial"), &all, 12)?;
    let read = operation("source-result", "filesystem.read_text");
    policy.record_results(
        std::slice::from_ref(&read),
        &[Ok(completed_operation(
            "source-result",
            ToolName::FilesystemReadText,
        ))],
    );
    policy.complete_advisory_stage(&stage_completion("read-source"))?;

    let mut revision = semantic_v6_plan_call("revise");
    revision.arguments.insert(
        "assumptions".to_owned(),
        serde_json::json!(["The source location and format are now established"]),
    );
    *revision
        .arguments
        .get_mut("steps")
        .and_then(|steps| steps.pointer_mut("/0/suggestedTools"))
        .ok_or_else(IpcError::internal)? = serde_json::json!(["filesystem.inspect_image"]);

    let unchanged = policy.accept(&revision, &all, 10)?;
    assert_eq!(unchanged["planUnchanged"], true);
    assert_eq!(unchanged["continuation"], "continue_current_stage");
    assert_eq!(unchanged["learnedFactsBelongInStageResults"], true);
    assert_eq!(unchanged["nextStageId"], "write-output");
    assert_eq!(policy.revision(), 1);
    assert_eq!(policy.revisions_used, 0);
    assert!(policy.forecast_steps[0].complete());
    Ok(())
}

#[test]
fn semantic_v6_allows_alternative_and_incomplete_operations_without_blocking()
-> Result<(), IpcError> {
    let all = vec![
        tool("filesystem.inspect_image"),
        tool("filesystem.read_text"),
        collector_tool(),
        tool("filesystem.write_text"),
        tool("computation.run"),
        tool(PLAN_TOOL),
        tool(COMPLETE_STAGE_TOOL),
    ];
    let mut policy = PlanPolicy::default();
    policy.accept(&semantic_v6_plan_call("initial"), &all, 12)?;

    let discovery = operation("located-path", "filesystem.find_paths");
    policy.record_results(
        std::slice::from_ref(&discovery),
        &[Ok(completed_find_paths_operation(
            "located-path",
            &[(1, false)],
        ))],
    );
    assert_eq!(
        policy.forecast_steps[0].semantic_completed_call_ids,
        ["located-path"]
    );

    policy.load_task_tools(["computation.run".to_owned()]);
    let unrelated = operation("support", "computation.run");
    policy.validate_calls(std::slice::from_ref(&unrelated))?;
    policy.record_results(
        std::slice::from_ref(&unrelated),
        &[Ok(completed_operation("support", ToolName::ComputationRun))],
    );
    assert_eq!(policy.primary_ready_step_index(), Some(0));

    let failed_suggestion = operation("failed-read", "filesystem.read_text");
    policy.validate_calls(std::slice::from_ref(&failed_suggestion))?;
    policy.record_results(
        std::slice::from_ref(&failed_suggestion),
        &[Err(IpcError::internal())],
    );
    assert_eq!(policy.primary_ready_step_index(), Some(0));
    assert!(
        policy
            .tools(&all)
            .iter()
            .any(|tool| tool.name == "filesystem.read_text")
    );

    let alternative = operation("alternative", "filesystem.inspect_image");
    policy.validate_calls(std::slice::from_ref(&alternative))?;
    policy.record_results(
        std::slice::from_ref(&alternative),
        &[Ok(completed_operation(
            "alternative",
            ToolName::FilesystemInspectImage,
        ))],
    );
    assert_eq!(policy.primary_ready_step_index(), Some(0));
    assert_eq!(
        policy.forecast_steps[0].semantic_completed_call_ids,
        ["located-path", "support", "alternative"]
    );
    let completed = policy.complete_advisory_stage(&stage_completion("read-source"))?;
    assert_eq!(completed["accepted"], true);
    assert_eq!(completed["nextStageId"], "write-output");
    assert_eq!(policy.primary_ready_step_index(), Some(1));
    assert!(
        policy
            .tools(&all)
            .iter()
            .any(|tool| tool.name == "computation.run")
    );
    Ok(())
}

#[test]
fn semantic_v6_reconciles_a_verified_write_immediately_before_completion() -> Result<(), IpcError> {
    let all = vec![
        tool("filesystem.read_text"),
        tool("filesystem.write_text"),
        tool(PLAN_TOOL),
        tool(COMPLETE_STAGE_TOOL),
    ];
    let mut policy = PlanPolicy::default();
    policy.accept(&semantic_v6_plan_call("initial"), &all, 12)?;

    let read = operation("source-result", "filesystem.read_text");
    policy.record_results(
        std::slice::from_ref(&read),
        &[Ok(completed_operation(
            "source-result",
            ToolName::FilesystemReadText,
        ))],
    );

    // Reproduce the retained ORCH-001 batch: the model emits the verified
    // outcome first, then explicitly completes the read stage.
    let write = operation("verified-write", "filesystem.write_text");
    policy.record_results(
        std::slice::from_ref(&write),
        &[Ok(completed_operation(
            "verified-write",
            ToolName::FilesystemWriteText,
        ))],
    );
    assert!(policy.planned_outcome_satisfied());
    assert!(!policy.forecast_complete());
    assert_eq!(policy.primary_ready_step_index(), Some(0));

    let completed = policy.complete_advisory_stage(&stage_completion("read-source"))?;
    assert_eq!(completed["stageId"], "read-source");
    assert!(completed["nextStageId"].is_null());
    assert!(policy.forecast_complete());
    assert!(!policy.final_response_ready());
    assert!(!policy.tools(&all).is_empty());
    assert!(!policy.allows_automatic_write_confirmation());
    assert_eq!(policy.verified_outcome_call_ids, ["verified-write"]);
    assert_eq!(
        policy.forecast_steps[1].semantic_completed_call_ids,
        ["verified-write"]
    );
    Ok(())
}

#[test]
fn write_advance_keeps_tools_and_new_results_for_another_deliverable_in_the_same_stage()
-> Result<(), IpcError> {
    let all = vec![
        tool("filesystem.read_text"),
        tool("filesystem.write_text"),
        tool(PLAN_TOOL),
        tool(COMPLETE_STAGE_TOOL),
    ];
    let mut policy = PlanPolicy::default();
    policy.accept(&semantic_v6_plan_call("initial"), &all, 12)?;
    policy.complete_advisory_stage(&stage_completion("read-source"))?;
    let write = operation("first-output", "filesystem.write_text");
    policy.record_results(
        &[write],
        &[Ok(completed_operation(
            "first-output",
            ToolName::FilesystemWriteText,
        ))],
    );
    assert!(policy.forecast_complete());
    assert!(!policy.final_response_ready());
    assert!(policy.current_stage_id().is_none());
    let read = operation("second-source", "filesystem.read_text");
    policy.validate_calls(std::slice::from_ref(&read))?;
    policy.record_results(
        &[read],
        &[Ok(completed_operation(
            "second-source",
            ToolName::FilesystemReadText,
        ))],
    );
    assert!(
        policy
            .retained_advisory_call_ids
            .contains(&"second-source".to_owned())
    );
    let write = operation("second-output", "filesystem.write_text");
    policy.validate_calls(std::slice::from_ref(&write))?;
    policy.record_results(
        &[write],
        &[Ok(completed_operation(
            "second-output",
            ToolName::FilesystemWriteText,
        ))],
    );
    assert_eq!(
        policy.verified_outcome_call_ids,
        ["first-output", "second-output"]
    );
    assert!(!policy.allows_automatic_write_confirmation());
    Ok(())
}

#[test]
fn semantic_v6_revision_preserves_same_id_checkpoint_but_reopens_replacement()
-> Result<(), IpcError> {
    let all = vec![
        tool("filesystem.read_text"),
        tool("filesystem.write_text"),
        tool(PLAN_TOOL),
        tool(COMPLETE_STAGE_TOOL),
    ];
    let mut policy = PlanPolicy::default();
    policy.accept(&semantic_v6_plan_call("initial"), &all, 12)?;

    let read = operation("located-source", "filesystem.read_text");
    policy.record_results(
        std::slice::from_ref(&read),
        &[Ok(completed_operation(
            "located-source",
            ToolName::FilesystemReadText,
        ))],
    );
    policy.complete_advisory_stage(&stage_completion("read-source"))?;

    let mut revision = semantic_v6_plan_call("revise");
    revision.arguments.insert(
        "steps".to_owned(),
        serde_json::json!([
            {
                "actionClass":"read", "dependsOn":[],
                "doneWhen":"The source path and format are available", "id":"read-source",
                "objective":"Confirm the source path and format",
                "suggestedTools":["filesystem.read_text"]
            },
            {
                "actionClass":"workspace_write", "dependsOn":["read-source"],
                "doneWhen":"The requested file exists", "id":"write-output",
                "objective":"Create the requested file",
                "suggestedTools":["filesystem.write_text"]
            }
        ]),
    );
    policy.accept(&revision, &all, 10)?;

    assert_eq!(policy.primary_ready_step_index(), Some(1));
    assert!(policy.forecast_steps[0].semantic_complete);
    assert_eq!(
        policy.forecast_steps[0].semantic_completed_call_ids,
        ["located-source"]
    );

    revision.call_id = "semantic-v6-plan-revise-renamed".to_owned();
    revision.arguments.insert(
        "steps".to_owned(),
        serde_json::json!([
            {
                "actionClass":"read", "dependsOn":[],
                "doneWhen":"The source path and format are available", "id":"locate-source",
                "objective":"Confirm the source path and format",
                "suggestedTools":["filesystem.read_text"]
            },
            {
                "actionClass":"workspace_write", "dependsOn":["locate-source"],
                "doneWhen":"The requested file exists", "id":"write-output",
                "objective":"Create the requested file",
                "suggestedTools":["filesystem.write_text"]
            }
        ]),
    );
    policy.accept(&revision, &all, 8)?;
    assert_eq!(policy.primary_ready_step_index(), Some(0));
    assert!(!policy.forecast_steps[0].semantic_complete);
    assert!(
        policy.forecast_steps[0]
            .semantic_completed_call_ids
            .is_empty()
    );
    assert_eq!(policy.retained_advisory_call_ids, ["located-source"]);
    Ok(())
}

#[test]
fn semantic_v6_accepts_standalone_advisory_stage_completion_without_a_receipt()
-> Result<(), IpcError> {
    let all = vec![
        tool("filesystem.read_text"),
        tool("filesystem.write_text"),
        tool(PLAN_TOOL),
        tool(COMPLETE_STAGE_TOOL),
    ];
    let mut policy = PlanPolicy::default();
    policy.accept(&semantic_v6_plan_call("initial"), &all, 12)?;

    let completed = policy.complete_advisory_stage(&stage_completion("read-source"))?;
    assert_eq!(completed["accepted"], true);
    assert_eq!(completed["stageId"], "read-source");
    assert_eq!(completed["nextStageId"], "write-output");
    assert_eq!(policy.primary_ready_step_index(), Some(1));
    assert!(!policy.final_response_ready());
    Ok(())
}

#[test]
fn semantic_v6_image_stage_completion_needs_no_prose_or_immediate_consumer() -> Result<(), IpcError>
{
    let all = vec![
        tool("filesystem.inspect_image"),
        tool("filesystem.write_text"),
        tool("filesystem.write_delimited_table"),
        tool(PLAN_TOOL),
        tool(COMPLETE_STAGE_TOOL),
    ];
    let mut plan = semantic_v6_plan_call("initial");
    let steps = plan
        .arguments
        .get_mut("steps")
        .ok_or_else(IpcError::internal)?;
    steps[0]["suggestedTools"] = serde_json::json!(["filesystem.inspect_image"]);
    let mut policy = PlanPolicy::default();
    policy.accept(&plan, &all, 12)?;
    let initial_surface = policy.tools(&all);
    assert!(
        initial_surface
            .iter()
            .any(|tool| tool.name == "filesystem.write_delimited_table")
    );
    let without_writer = all
        .iter()
        .filter(|tool| tool.name != "filesystem.write_delimited_table")
        .cloned()
        .collect::<Vec<_>>();
    assert!(
        !policy
            .tools(&without_writer)
            .iter()
            .any(|tool| tool.name == "filesystem.write_delimited_table")
    );
    let image = operation("image-source", "filesystem.inspect_image");
    policy.record_results(
        std::slice::from_ref(&image),
        &[Ok(completed_operation(
            "image-source",
            ToolName::FilesystemInspectImage,
        ))],
    );

    assert!(policy.retained_advisory_call_ids.is_empty());
    let completed = policy.complete_advisory_stage(&stage_completion("read-source"))?;
    assert_eq!(completed["accepted"], true);
    assert_eq!(completed["nextStageId"], "write-output");
    assert_eq!(policy.primary_ready_step_index(), Some(1));
    assert_eq!(policy.tools(&all), initial_surface);
    Ok(())
}

#[test]
fn semantic_v6_write_completion_still_requires_verified_outcome_for_final_response()
-> Result<(), IpcError> {
    let all = vec![
        tool("filesystem.read_text"),
        tool("filesystem.write_text"),
        tool(PLAN_TOOL),
        tool(COMPLETE_STAGE_TOOL),
    ];
    let mut policy = PlanPolicy::default();
    policy.accept(&semantic_v6_plan_call("initial"), &all, 12)?;

    policy.complete_advisory_stage(&stage_completion("read-source"))?;
    let completed = policy.complete_advisory_stage(&stage_completion("write-output"))?;

    assert_eq!(completed["accepted"], true);
    assert!(completed["nextStageId"].is_null());
    assert!(policy.forecast_complete());
    assert!(!policy.final_response_ready());
    assert!(!policy.planned_outcome_satisfied());
    assert_eq!(
        policy.current_step_relevant_tools(&all),
        ["filesystem.write_text", PLAN_TOOL]
    );
    Ok(())
}

#[test]
fn semantic_v3_does_not_rebind_one_receipt_to_two_roles() -> Result<(), IpcError> {
    let all = vec![
        tool("filesystem.inspect_image"),
        tool("filesystem.write_text"),
        tool(PLAN_TOOL),
        tool(FORECAST_TOOL),
    ];
    let mut plan = semantic_v3_plan_call("initial");
    plan.arguments.insert(
        "steps".to_owned(),
        serde_json::json!([
            {
                "actionClass":"read","dependsOn":[],"doneWhen":"Source loaded","id":"source","objective":"Load source",
                "obligations":[{"id":"source-image","roleKind":"source","evidenceClass":"semantic_content","postcondition":"image_content_loaded"}]
            },
            {
                "actionClass":"workspace_write","dependsOn":["source"],"doneWhen":"Output verified","id":"write","objective":"Write output",
                "obligations":[
                    {"id":"grounded-source","roleKind":"source","evidenceClass":"semantic_content","postcondition":"image_content_loaded"},
                    {"id":"grounded-convention","roleKind":"convention","evidenceClass":"semantic_content","postcondition":"content_loaded"},
                    {"id":"written-target","roleKind":"target","evidenceClass":"identity_location","postcondition":"file_write_verified"}
                ]
            }
        ]),
    );
    let mut forecast = semantic_v3_forecast_call("initial");
    forecast.arguments.insert(
        "entries".to_owned(),
        serde_json::json!([
            {"stepId":"source","category":"workspace_read","purpose":"Load source","dependsOn":[],"requiredObligations":["source-image"],"postcondition":"content_loaded","routes":[{"operation":"filesystem.inspect_image","knownArguments":[],"unresolvedArguments":[]}]},
            {"stepId":"write","category":"workspace_write","purpose":"Write output","dependsOn":["source"],"requiredObligations":["grounded-source","grounded-convention","written-target"],"postcondition":"file_write_verified","routes":[{"operation":"filesystem.write_text","knownArguments":[],"unresolvedArguments":[]}]}
        ]),
    );
    let mut policy = PlanPolicy::default();
    policy.accept(&plan, &all, 10)?;
    policy.accept_forecast(&forecast, &all)?;
    let image = operation("image-receipt", "filesystem.inspect_image");
    policy.validate_calls(std::slice::from_ref(&image))?;
    policy.record_results(
        std::slice::from_ref(&image),
        &[Ok(completed_operation(
            "image-receipt",
            ToolName::FilesystemInspectImage,
        ))],
    );

    assert!(policy.forecast_steps[0].complete());
    assert!(policy.v3_prerequisite_receipts(1).is_none());
    assert!(
        !policy
            .tools(&all)
            .iter()
            .any(|tool| tool.name == "filesystem.write_text")
    );
    let error = policy
        .validate_calls(&[operation("write", "filesystem.write_text")])
        .err()
        .ok_or_else(IpcError::internal)?;
    assert!(error.message.contains("grounded-convention"));
    Ok(())
}

#[test]
fn semantic_v3_collector_can_prove_distinct_structured_outputs() -> Result<(), IpcError> {
    let all = vec![
        tool("filesystem.collect_evidence"),
        tool(PLAN_TOOL),
        tool(FORECAST_TOOL),
    ];
    let mut plan = semantic_v3_plan_call("initial");
    plan.arguments
        .insert("outcome".to_owned(), serde_json::json!("answer"));
    plan.arguments.insert(
        "steps".to_owned(),
        serde_json::json!([{
            "actionClass":"read","dependsOn":[],"doneWhen":"Both exact resources are loaded","id":"evidence","objective":"Load source and convention",
            "obligations":[
                {"id":"source-image","roleKind":"source","evidenceClass":"semantic_content","postcondition":"image_content_loaded"},
                {"id":"convention-format","roleKind":"convention","evidenceClass":"format_structure","postcondition":"text_format_loaded"}
            ]
        }]),
    );
    let mut forecast = semantic_v3_forecast_call("initial");
    forecast.arguments.insert(
        "entries".to_owned(),
        serde_json::json!([{
            "stepId":"evidence","category":"workspace_read","purpose":"Load exact evidence","dependsOn":[],
            "requiredObligations":["source-image","convention-format"],"postcondition":"content_loaded",
            "routes":[{"operation":"filesystem.collect_evidence","knownArguments":[],"unresolvedArguments":[]}]
        }]),
    );
    let mut policy = PlanPolicy::default();
    policy.accept(&plan, &all, 8)?;
    policy.accept_forecast(&forecast, &all)?;
    let mut collector = typed_collector_operation("collector", true);
    collector.arguments.insert(
        "items".to_owned(),
        serde_json::json!([
            {"id":"source-image","operation":"load_exact_image_semantic"},
            {"id":"convention-format","operation":"load_exact_text_format"}
        ]),
    );
    policy.validate_calls(std::slice::from_ref(&collector))?;
    policy.record_results(
        std::slice::from_ref(&collector),
        &[Ok(completed_typed_collector_operation("collector"))],
    );

    assert!(policy.forecast_steps[0].complete());
    let receipts = &policy.forecast_steps[0].satisfied_obligations;
    assert_eq!(receipts.len(), 2);
    assert_ne!(
        receipts["source-image"].output_key,
        receipts["convention-format"].output_key
    );
    Ok(())
}

#[test]
fn semantic_v2_requires_one_forecast_boundary_and_repairs_the_first_nested_defect()
-> Result<(), IpcError> {
    let all = vec![
        tool(PLAN_TOOL),
        tool(FORECAST_TOOL),
        tool("filesystem.inspect_image"),
        tool("filesystem.write_text"),
    ];
    let mut policy = PlanPolicy::default();
    policy.accept(&semantic_plan_call("initial"), &all, 8)?;
    assert!(policy.forecast_required());
    assert_eq!(policy.tools(&all)[0].name, FORECAST_TOOL);

    let mut malformed = semantic_forecast_call("initial");
    malformed
        .arguments
        .get_mut("entries")
        .ok_or_else(IpcError::internal)?[0]["requiredEvidence"] = serde_json::json!([]);
    let Err(error) = policy.accept_forecast(&malformed, &all) else {
        return Err(IpcError::internal());
    };
    assert!(error.retryable);
    assert!(error.message.contains("$.entries[0].requiredEvidence"));
    assert!(policy.forecast_required());

    policy.accept_forecast(&semantic_forecast_call("initial"), &all)?;
    assert!(!policy.forecast_required());
    assert_eq!(policy.forecast_revision(), 1);
    Ok(())
}

#[test]
fn evidence_can_revise_advisory_routes_without_revising_the_semantic_plan() -> Result<(), IpcError>
{
    let all = vec![
        tool(PLAN_TOOL),
        tool(FORECAST_TOOL),
        tool("filesystem.inspect_image"),
        tool("filesystem.read_text"),
        tool("filesystem.write_text"),
    ];
    let mut policy = PlanPolicy::default();
    policy.accept(&semantic_plan_call("initial"), &all, 8)?;
    policy.accept_forecast(&semantic_forecast_call("initial"), &all)?;
    let plan_revision = policy.revision();

    let mut revised = semantic_forecast_call("revise");
    revised
        .arguments
        .get_mut("entries")
        .ok_or_else(IpcError::internal)?[0]["routes"][0]["operation"] =
        serde_json::json!("filesystem.read_text");
    policy.accept_forecast(&revised, &all)?;
    assert_eq!(policy.revision(), plan_revision);
    assert_eq!(policy.forecast_revision(), 2);
    assert_eq!(
        policy.forecast_diagnostic(&operation("read", "filesystem.read_text")),
        ("matched", Some("workspace_read"))
    );
    Ok(())
}

#[test]
fn category_projection_expands_by_forecast_revision_and_safe_read_escape() -> Result<(), IpcError> {
    let all = vec![
        tool(PLAN_TOOL),
        tool(FORECAST_TOOL),
        tool("filesystem.find_paths"),
        tool("filesystem.inspect_image"),
        tool("filesystem.read_text"),
        tool("filesystem.write_text"),
    ];
    let mut policy = PlanPolicy::default();
    policy.accept(&semantic_plan_call("initial"), &all, 10)?;
    policy.accept_forecast(&semantic_forecast_call("initial"), &all)?;
    let initial = policy
        .tools(&all)
        .into_iter()
        .map(|tool| tool.name)
        .collect::<BTreeSet<_>>();
    assert!(initial.contains("filesystem.inspect_image"));
    assert!(!initial.contains("filesystem.find_paths"));
    assert!(!initial.contains("filesystem.write_text"));

    let discovery = operation("discover", "filesystem.find_paths");
    assert!(policy.authorize_safe_read_recovery(std::slice::from_ref(&discovery), &all));
    assert!(
        policy
            .tools(&all)
            .iter()
            .any(|tool| tool.name == "filesystem.find_paths")
    );

    let mut revised = semantic_forecast_call("revise");
    let source = revised
        .arguments
        .get_mut("entries")
        .and_then(serde_json::Value::as_array_mut)
        .and_then(|entries| entries.first_mut())
        .ok_or_else(IpcError::internal)?;
    source["category"] = serde_json::json!("workspace_discovery");
    source["postcondition"] = serde_json::json!("evidence_ready");
    source["routes"][0]["operation"] = serde_json::json!("filesystem.find_paths");
    policy.accept_forecast(&revised, &all)?;
    let expanded = policy
        .tools(&all)
        .into_iter()
        .map(|tool| tool.name)
        .collect::<BTreeSet<_>>();
    assert!(expanded.contains("filesystem.find_paths"));
    assert!(!expanded.contains("filesystem.inspect_image"));
    Ok(())
}
