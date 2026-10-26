use super::*;

#[test]
fn incomplete_collector_is_refinable_and_host_binds_completed_receipts() -> Result<(), IpcError> {
    let all = [
        tool(PLAN_TOOL),
        tool("filesystem.collect_evidence"),
        tool("orchestration.transform_artifact"),
    ];
    let plan = collector_artifact_plan();
    let mut policy = PlanPolicy::default();
    policy.accept(&plan, &all, 8)?;

    let incomplete = typed_collector_operation("evidence-incomplete", true);
    policy.record_results(
        std::slice::from_ref(&incomplete),
        &[Ok(incomplete_evidence_operation("evidence-incomplete"))],
    );
    assert_eq!(
        policy.current_step_unfinished_tools(),
        vec!["filesystem.collect_evidence"]
    );
    assert!(policy.completed_evidence.is_empty());
    assert!(policy.forecast_steps[0].blocked);
    assert_eq!(policy.working_allowance(), 5);
    assert_eq!(
        policy
            .progress_projection(false)
            .ok_or_else(|| IpcError::invalid_request("missing incomplete projection"))?,
        incomplete_collector_projection()
    );
    policy.validate_calls(std::slice::from_ref(&incomplete))?;

    let discovery_only = typed_collector_operation("evidence-discovery", false);
    policy.record_results(
        std::slice::from_ref(&discovery_only),
        &[Ok(completed_operation(
            "evidence-discovery",
            ToolName::FilesystemCollectEvidence,
        ))],
    );
    assert_eq!(
        policy.current_step_unfinished_tools(),
        vec!["filesystem.collect_evidence"]
    );
    assert!(policy.completed_evidence.is_empty());
    assert_eq!(policy.working_allowance(), 6);
    assert_eq!(
        policy
            .unresolved_evidence_call_ids
            .get("evidence")
            .map(String::as_str),
        Some("evidence-discovery")
    );

    let third_incomplete = typed_collector_operation("evidence-third", true);
    policy.record_results(
        std::slice::from_ref(&third_incomplete),
        &[Ok(incomplete_evidence_operation("evidence-third"))],
    );
    assert_eq!(
        policy.working_allowance(),
        6,
        "refinement headroom remains bounded after two useful incomplete results"
    );
    assert_eq!(
        policy
            .unresolved_evidence_call_ids
            .get("evidence")
            .map(String::as_str),
        Some("evidence-third")
    );

    let complete = typed_collector_operation("evidence-ready", true);
    policy.record_results(
        std::slice::from_ref(&complete),
        &[Ok(completed_operation(
            "evidence-ready",
            ToolName::FilesystemCollectEvidence,
        ))],
    );
    assert_eq!(
        policy.current_step_unfinished_tools(),
        vec!["orchestration.transform_artifact"]
    );
    assert!(policy.unresolved_evidence_call_ids.is_empty());

    let mut transform = artifact_operation("transform", "invented", "invented");
    policy.bind_artifact_evidence(std::slice::from_mut(&mut transform));
    assert_eq!(
        transform.arguments["evidence"],
        serde_json::json!([
            {"callId":"evidence-ready","role":"source"},
            {"callId":"evidence-ready","role":"convention"}
        ])
    );
    policy.validate_calls(std::slice::from_ref(&transform))?;
    Ok(())
}

#[test]
fn plan_revision_retains_the_latest_unresolved_evidence_receipt() -> Result<(), IpcError> {
    let all = [
        tool(PLAN_TOOL),
        tool("filesystem.collect_evidence"),
        tool("orchestration.transform_artifact"),
    ];
    let mut initial = plan_call("initial", &["orchestration.transform_artifact"]);
    initial
        .arguments
        .insert("outcome".to_owned(), serde_json::json!("create_file"));
    initial.arguments.insert(
        "steps".to_owned(),
        serde_json::json!([
            {
                "completionEvidence":"Candidate paths returned",
                "expectedCalls":[{
                    "purpose":"Discover candidates",
                    "providesEvidence":[
                        {"role":"source","evidenceClass":"semantic_content"},
                        {"role":"convention","evidenceClass":"format_structure"}
                    ],
                    "tool":"filesystem.collect_evidence"
                }],
                "id":"evidence","purpose":"Collect evidence",
                "requiredEvidence":["Source and convention"]
            },
            {
                "completionEvidence":"Artifact verified",
                "expectedCalls":[{"purpose":"Create artifact","tool":"orchestration.transform_artifact"}],
                "id":"write","purpose":"Create artifact","requiredEvidence":["Typed evidence"]
            }
        ]),
    );
    let mut policy = PlanPolicy::default();
    policy.accept(&initial, &all, 8)?;
    let incomplete = typed_collector_operation("candidate-result", true);
    policy.record_results(
        std::slice::from_ref(&incomplete),
        &[Ok(incomplete_evidence_operation("candidate-result"))],
    );

    let mut revision = initial;
    revision.call_id = "revision".to_owned();
    revision
        .arguments
        .insert("mode".to_owned(), serde_json::json!("revise"));
    policy.accept(&revision, &all, 8)?;

    assert_eq!(
        policy
            .unresolved_evidence_call_ids
            .get("evidence")
            .map(String::as_str),
        Some("candidate-result")
    );
    assert_eq!(
        policy
            .progress_projection(false)
            .ok_or_else(|| IpcError::invalid_request("missing revised projection"))?,
        ToolPlanProgress::new(
            2,
            0,
            vec![
                ToolPlanStepProgress::new(
                    1,
                    ToolPlanStepStatus::NotStarted,
                    vec!["filesystem.collect_evidence".to_owned()],
                )
                .with_contract(
                    "evidence".to_owned(),
                    "Collect evidence".to_owned(),
                    vec!["Source and convention".to_owned()],
                    "Candidate paths returned".to_owned(),
                    Vec::new(),
                ),
                ToolPlanStepProgress::new(
                    2,
                    ToolPlanStepStatus::NotStarted,
                    vec!["orchestration.transform_artifact".to_owned()],
                )
                .with_contract(
                    "write".to_owned(),
                    "Create artifact".to_owned(),
                    vec!["Typed evidence".to_owned()],
                    "Artifact verified".to_owned(),
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
        .with_retained_call_ids(["candidate-result".to_owned()])
    );
    Ok(())
}

#[test]
fn identity_only_discovery_cannot_claim_structural_convention_evidence() -> Result<(), IpcError> {
    let all = [
        tool(PLAN_TOOL),
        tool("filesystem.find_paths"),
        tool("filesystem.inspect_image"),
        tool("orchestration.transform_artifact"),
    ];
    let mut plan = plan_call("initial", &["orchestration.transform_artifact"]);
    plan.arguments
        .insert("outcome".to_owned(), serde_json::json!("create_file"));
    plan.arguments.insert(
        "steps".to_owned(),
        serde_json::json!([
            {
                "completionEvidence":"Convention path found",
                "expectedCalls":[{
                    "purpose":"Find a reconstructed table",
                    "providesEvidence":[{"role":"convention","evidenceClass":"format_structure"}],
                    "tool":"filesystem.find_paths"
                }],
                "id":"convention","purpose":"Find convention","requiredEvidence":["Convention example"]
            },
            {
                "completionEvidence":"Source loaded",
                "expectedCalls":[{
                    "purpose":"Inspect the exact source",
                    "providesEvidence":[{"role":"source","evidenceClass":"semantic_content"}],
                    "tool":"filesystem.inspect_image"
                }],
                "id":"source","purpose":"Load source","requiredEvidence":["Source values"]
            },
            {
                "completionEvidence":"Artifact verified",
                "expectedCalls":[{"purpose":"Create artifact","tool":"orchestration.transform_artifact"}],
                "id":"artifact","purpose":"Create artifact","requiredEvidence":["Typed source and convention evidence"]
            }
        ]),
    );

    let Err(error) = PlanPolicy::default().accept(&plan, &all, 8) else {
        return Err(IpcError::invalid_request(
            "path discovery claimed format evidence",
        ));
    };
    assert!(error.message.contains("filesystem.find_paths"));
    assert!(error.message.contains("at most identity_location"));
    Ok(())
}

#[test]
fn malformed_evidence_provisions_receive_precise_aggregated_diagnostics() -> Result<(), IpcError> {
    let all = [tool(PLAN_TOOL), tool("filesystem.read_text")];
    let mut plan = plan_call("initial", &["filesystem.read_text"]);
    plan.arguments.insert(
        "steps".to_owned(),
        serde_json::json!([{
            "completionEvidence":"Document loaded",
            "expectedCalls":[{
                "purpose":"Read a document",
                "providesEvidence":[{"role":"example","evidenceClass":"semantic_content"}],
                "tool":"filesystem.read_text"
            }],
            "id":"read","purpose":"Read document","requiredEvidence":["Document contents"]
        }]),
    );

    let Err(error) = PlanPolicy::default().accept(&plan, &all, 8) else {
        return Err(IpcError::invalid_request(
            "unknown evidence role was accepted",
        ));
    };
    assert!(error.message.contains("independently detectable"));
    assert!(error.message.contains("unknown role"));
    assert!(!error.message.contains("invalid provisional plan arguments"));
    Ok(())
}

#[test]
fn semantic_v6_admission_preserves_exact_array_diagnostics_from_metadata_boundary()
-> Result<(), IpcError> {
    let all = [
        tool(PLAN_TOOL),
        tool("filesystem.find_paths"),
        tool("filesystem.read_text"),
        tool("filesystem.write_text"),
    ];
    let mut plan = semantic_v6_plan_call("initial");
    plan.arguments.insert(
        "constraints".to_owned(),
        serde_json::json!("Preserve source values"),
    );
    let Some(steps) = plan
        .arguments
        .get_mut("steps")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return Err(IpcError::internal());
    };
    steps[1]["suggestedTools"] = serde_json::json!(["filesystem.unavailable"]);

    let error = PlanPolicy::default()
        .accept(&plan, &all, 8)
        .err()
        .ok_or_else(IpcError::internal)?;
    assert!(error.message.contains("/constraints"));
    assert!(error.message.contains("/steps/1/suggestedTools/0"));
    assert!(!error.message.contains("invalid provisional plan arguments"));
    assert_eq!(
        plan_repair_defects(&error)
            .ok_or_else(IpcError::internal)?
            .len(),
        2
    );
    Ok(())
}

#[test]
fn every_supported_plan_size_has_the_same_generic_progress_projection() -> Result<(), IpcError> {
    let all = [tool(PLAN_TOOL), tool("filesystem.read_text")];
    for count in 1..=MAX_PROVISIONAL_PLAN_STEPS {
        let mut policy = PlanPolicy::default();
        policy.accept(&plan_with_step_count(count), &all, 18)?;
        let projection = policy
            .progress_projection(false)
            .ok_or_else(|| IpcError::invalid_request("missing plan projection"))?;
        let expected_steps = (1..=count)
            .map(|ordinal| {
                standard_step_progress(
                    ordinal,
                    ToolPlanStepStatus::NotStarted,
                    vec!["filesystem.read_text".to_owned()],
                    Vec::new(),
                )
            })
            .collect();
        let expected_allowance = count
            .saturating_mul(2)
            .max(count.saturating_add(PLAN_RESERVE_CALLS))
            .min(18);
        assert_eq!(
            projection,
            ToolPlanProgress::new(
                1,
                0,
                expected_steps,
                Some(1),
                ToolPlanOutcome::Other,
                false,
                false,
                expected_allowance,
                false,
                0,
            )
        );
    }
    Ok(())
}

#[test]
fn completed_calls_advance_the_advisory_cursor_without_claiming_semantic_completion()
-> Result<(), IpcError> {
    let all = [tool(PLAN_TOOL), tool("filesystem.read_text")];
    let mut policy = PlanPolicy::default();
    policy.accept(&plan_with_step_count(2), &all, 18)?;
    policy.record_results(
        &[operation("read", "filesystem.read_text")],
        &[Ok(completed_operation(
            "read",
            ToolName::FilesystemReadText,
        ))],
    );

    assert_eq!(
        policy.progress_projection(false),
        Some(ToolPlanProgress::new(
            1,
            0,
            vec![
                standard_step_progress(
                    1,
                    ToolPlanStepStatus::ForecastComplete,
                    Vec::new(),
                    vec!["read".to_owned()],
                ),
                standard_step_progress(
                    2,
                    ToolPlanStepStatus::NotStarted,
                    vec!["filesystem.read_text".to_owned()],
                    Vec::new(),
                ),
            ],
            Some(2),
            ToolPlanOutcome::Other,
            false,
            false,
            3,
            false,
            0,
        ))
    );
    Ok(())
}

#[test]
fn forecast_matching_preserves_declared_purpose_and_does_not_skip_steps() -> Result<(), IpcError> {
    let all = [
        tool(PLAN_TOOL),
        tool("filesystem.inspect_image"),
        tool("filesystem.write_text"),
    ];
    let mut plan = plan_with_step_count(2);
    plan.arguments.insert(
        "steps".to_owned(),
        serde_json::json!([
            {
                "completionEvidence": "The source image is available",
                "expectedCalls": [{
                    "purpose": "Load the source values",
                    "tool": "filesystem.inspect_image"
                }],
                "id": "inspect",
                "purpose": "Read the source",
                "requiredEvidence": ["Source image"]
            },
            {
                "completionEvidence": "The target file is written",
                "expectedCalls": [{
                    "purpose": "Create the requested artifact",
                    "tool": "filesystem.write_text"
                }],
                "id": "write",
                "purpose": "Produce the outcome",
                "requiredEvidence": ["Transformed content"]
            }
        ]),
    );
    let mut policy = PlanPolicy::default();
    policy.accept(&plan, &all, 18)?;
    assert_eq!(
        policy.current_step_unfinished_tools(),
        vec!["filesystem.inspect_image"]
    );
    assert_eq!(policy.unfinished_call_count(), 2);
    assert!(!policy.forecast_complete());
    assert_eq!(
        policy.forecast_steps[0].calls[0].purpose,
        "Load the source values"
    );
    assert_eq!(
        policy.diagnostic_match(&operation("inspect", "filesystem.inspect_image")),
        (1, Some(1), ForecastDiagnosticMatch::Matched)
    );
    assert_eq!(
        policy.diagnostic_match(&operation("write", "filesystem.write_text")),
        (1, Some(2), ForecastDiagnosticMatch::OutOfOrder)
    );
    assert_eq!(
        policy.diagnostic_match(&operation("stat", "filesystem.stat")),
        (1, Some(1), ForecastDiagnosticMatch::Unforecasted)
    );

    policy.record_results(
        &[operation("early-write", "filesystem.write_text")],
        &[Ok(completed_operation(
            "early-write",
            ToolName::FilesystemWriteText,
        ))],
    );
    assert!(!policy.forecast_steps[0].calls[0].completed);
    assert!(!policy.forecast_steps[1].calls[0].completed);
    assert_eq!(policy.unforecasted_operations, 1);

    policy.record_results(
        &[operation("inspect", "filesystem.inspect_image")],
        &[Ok(completed_operation(
            "inspect",
            ToolName::FilesystemInspectImage,
        ))],
    );
    assert_eq!(
        policy.current_step_unfinished_tools(),
        vec!["filesystem.write_text"]
    );
    assert_eq!(policy.unfinished_call_count(), 1);

    policy.record_results(
        &[operation("write", "filesystem.write_text")],
        &[Ok(completed_operation(
            "write",
            ToolName::FilesystemWriteText,
        ))],
    );
    assert!(
        policy
            .forecast_steps
            .iter()
            .all(|step| step.calls.iter().all(|call| call.completed))
    );
    assert!(policy.forecast_complete());
    assert!(policy.current_step_unfinished_tools().is_empty());
    Ok(())
}

#[test]
fn progress_handles_unplanned_failures_parallel_success_and_revision_without_resetting_facts()
-> Result<(), IpcError> {
    let all = [
        tool(PLAN_TOOL),
        tool("filesystem.list"),
        tool("filesystem.read_text"),
    ];
    let mut policy = PlanPolicy::default();
    policy.accept(&plan_with_step_count(3), &all, 18)?;

    policy.record_results(
        &[operation("unplanned", "filesystem.list")],
        &[Ok(completed_operation(
            "unplanned",
            ToolName::FilesystemList,
        ))],
    );
    policy.record_results(
        &[operation("failed", "filesystem.read_text")],
        &[Ok(failed_operation("failed", ToolName::FilesystemReadText))],
    );
    let blocked = policy
        .progress_projection(false)
        .ok_or_else(|| IpcError::invalid_request("missing blocked projection"))?;
    assert_eq!(
        blocked,
        ToolPlanProgress::new(
            1,
            0,
            vec![
                standard_step_progress(
                    1,
                    ToolPlanStepStatus::Blocked,
                    vec!["filesystem.read_text".to_owned()],
                    Vec::new(),
                ),
                standard_step_progress(
                    2,
                    ToolPlanStepStatus::NotStarted,
                    vec!["filesystem.read_text".to_owned()],
                    Vec::new(),
                ),
                standard_step_progress(
                    3,
                    ToolPlanStepStatus::NotStarted,
                    vec!["filesystem.read_text".to_owned()],
                    Vec::new(),
                ),
            ],
            Some(1),
            ToolPlanOutcome::Other,
            false,
            false,
            4,
            false,
            1,
        )
    );

    policy.record_results(
        &[
            operation("retry", "filesystem.read_text"),
            operation("parallel", "filesystem.read_text"),
        ],
        &[
            Ok(completed_operation("retry", ToolName::FilesystemReadText)),
            Ok(completed_operation(
                "parallel",
                ToolName::FilesystemReadText,
            )),
        ],
    );
    let mut revision = plan_with_step_count(1);
    revision
        .arguments
        .insert("mode".to_owned(), serde_json::json!("revise"));
    policy.accept(&revision, &all, 10)?;
    assert_eq!(
        policy.progress_projection(false),
        Some(ToolPlanProgress::new(
            2,
            2,
            vec![standard_step_progress(
                1,
                ToolPlanStepStatus::NotStarted,
                vec!["filesystem.read_text".to_owned()],
                Vec::new(),
            )],
            Some(1),
            ToolPlanOutcome::Other,
            false,
            false,
            3,
            false,
            1,
        ))
    );
    Ok(())
}
