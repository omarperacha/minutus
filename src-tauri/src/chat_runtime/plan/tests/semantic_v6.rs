use super::*;

fn external_write_plan() -> ParsedToolCall {
    ParsedToolCall {
        call_id: "external-plan".to_owned(),
        recipient: PLAN_TOOL.to_owned(),
        name: PLAN_TOOL.to_owned(),
        arguments: BTreeMap::from([
            ("assumptions".to_owned(), serde_json::json!([])),
            (
                "constraints".to_owned(),
                serde_json::json!(["Do not retry"]),
            ),
            (
                "contractVersion".to_owned(),
                serde_json::json!(crate::chat_tools::SEMANTIC_PLAN_CONTRACT_VERSION),
            ),
            ("mode".to_owned(), serde_json::json!("initial")),
            ("outcome".to_owned(), serde_json::json!("other")),
            (
                "steps".to_owned(),
                serde_json::json!([{
                    "actionClass":"external_write",
                    "dependsOn":[],
                    "doneWhen":"The diagnostic resource result is returned",
                    "id":"purchase-resource",
                    "objective":"Purchase the requested diagnostic resource once",
                    "suggestedTools":["x402.request"]
                }]),
            ),
            (
                "successCriteria".to_owned(),
                serde_json::json!(["The one permitted purchase attempt has a reported result"]),
            ),
            (
                "target".to_owned(),
                serde_json::json!("Purchase the operator-provided Flowzed diagnostic resource"),
            ),
        ]),
    }
}

#[test]
fn advisory_dependencies_are_optional_but_explicit_dependencies_remain_validated()
-> Result<(), IpcError> {
    let available = BTreeSet::from(["x402.request"]);
    let mut arguments = external_write_plan().arguments;
    arguments
        .get_mut("steps")
        .and_then(|steps| steps.pointer_mut("/0"))
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(IpcError::internal)?
        .remove("dependsOn");
    let parsed = SemanticPlanContract::parse(
        &arguments,
        &available,
        16,
        PlanContractVersion::SemanticExecutionV6,
    )?;
    assert!(parsed.steps[0].depends_on.is_empty());
    for invalid in [
        serde_json::Value::Null,
        serde_json::json!("[]"),
        serde_json::json!(["missing"]),
        serde_json::json!(["purchase-resource"]),
    ] {
        arguments
            .get_mut("steps")
            .and_then(|steps| steps.pointer_mut("/0"))
            .and_then(serde_json::Value::as_object_mut)
            .ok_or_else(IpcError::internal)?
            .insert("dependsOn".into(), invalid);
        assert!(
            SemanticPlanContract::parse(
                &arguments,
                &available,
                16,
                PlanContractVersion::SemanticExecutionV6,
            )
            .is_err()
        );
    }
    let stage = arguments
        .get_mut("steps")
        .and_then(|steps| steps.pointer_mut("/0"))
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(IpcError::internal)?;
    stage.remove("dependsOn");
    stage.remove("doneWhen");
    let error = SemanticPlanContract::parse(
        &arguments,
        &available,
        16,
        PlanContractVersion::SemanticExecutionV6,
    )
    .err()
    .ok_or_else(IpcError::internal)?;
    assert_eq!(error.message.matches("/steps/0/doneWhen").count(), 1);
    Ok(())
}

#[test]
fn semantic_v6_projects_external_write_without_a_forecast_round() -> Result<(), IpcError> {
    let all = vec![
        tool(PLAN_TOOL),
        tool(FORECAST_TOOL),
        tool(COMPLETE_STAGE_TOOL),
        tool("x402.request"),
    ];
    let mut policy = PlanPolicy::default();
    let accepted = policy.accept(&external_write_plan(), &all, 8)?;
    assert_eq!(accepted["accepted"], true);
    assert!(!policy.forecast_required());

    let projected = policy
        .tools(&all)
        .into_iter()
        .map(|tool| tool.name)
        .collect::<BTreeSet<_>>();
    assert!(projected.contains("x402.request"));
    assert!(projected.contains(COMPLETE_STAGE_TOOL));
    assert!(!projected.contains(FORECAST_TOOL));
    assert!(
        policy
            .validate_calls(&[operation("purchase", "x402.request")])
            .is_ok()
    );
    Ok(())
}

#[test]
fn advisory_read_work_can_select_multi_operation_tools_without_class_repairs()
-> Result<(), IpcError> {
    // Discovery and reading a computed dataset need not share the catalogue's
    // maximum operation class. Admission still loads only real available tools.
    for name in ["x402.request", "computation.run"] {
        let all = vec![tool(PLAN_TOOL), tool(COMPLETE_STAGE_TOOL), tool(name)];
        let mut plan = external_write_plan();
        plan.arguments.insert("steps".to_owned(), serde_json::json!([{
            "actionClass":"read", "dependsOn":[], "doneWhen":"The requested facts are available",
            "id":"discover", "objective":"Discover the information needed for the next action",
            "suggestedTools":[name]
        }]));
        let mut policy = PlanPolicy::default();
        assert_eq!(policy.accept(&plan, &all, 8)?["accepted"], true);
        assert!(policy.tools(&all).iter().any(|tool| tool.name == name));
        assert!(!policy.forecast_complete());
        assert!(!policy.final_response_ready());
        let mut unavailable = PlanPolicy::default();
        assert!(unavailable.accept(&plan, &[tool(PLAN_TOOL)], 8).is_err());
    }
    assert_eq!(
        crate::chat_tools::tool_action_class("x402.request"),
        Some(crate::chat_tools::ToolActionClass::ExternalWrite)
    );
    Ok(())
}

#[test]
fn authoritative_x402_failure_completes_its_one_shot_stage_for_reporting() -> Result<(), IpcError> {
    let all = vec![
        tool(PLAN_TOOL),
        tool(COMPLETE_STAGE_TOOL),
        tool("x402.request"),
    ];
    let mut policy = PlanPolicy::default();
    policy.accept(&external_write_plan(), &all, 8)?;
    let mut call = operation("purchase", "x402.request");
    call.arguments
        .insert("operation".to_owned(), serde_json::json!("purchase"));
    let mut failure = failed_operation("purchase", ToolName::X402Request);
    failure.activity.status = TaskActivityStatus::Failed;

    policy.record_results(std::slice::from_ref(&call), &[Ok(failure)]);

    assert!(policy.forecast_complete());
    assert!(policy.final_response_ready());
    assert!(policy.tools(&all).is_empty());
    Ok(())
}

#[test]
fn x402_discovery_keeps_the_purchase_stage_open() -> Result<(), IpcError> {
    let all = vec![
        tool(PLAN_TOOL),
        tool(COMPLETE_STAGE_TOOL),
        tool("x402.request"),
    ];
    let mut policy = PlanPolicy::default();
    policy.accept(&external_write_plan(), &all, 8)?;
    let mut call = operation("discover", "x402.request");
    call.arguments
        .insert("operation".to_owned(), serde_json::json!("discover"));
    let discovery = completed_operation("discover", ToolName::X402Request);

    policy.record_results(&[call], &[Ok(discovery)]);

    assert!(!policy.forecast_complete());
    assert!(!policy.final_response_ready());
    assert!(
        policy
            .tools(&all)
            .iter()
            .any(|tool| tool.name == "x402.request")
    );
    Ok(())
}

#[test]
fn create_file_outcome_requires_a_file_write_not_directory_creation() -> Result<(), IpcError> {
    let all = [
        tool(PLAN_TOOL),
        tool("filesystem.create_directory"),
        tool("filesystem.write_text"),
    ];
    let mut policy = PlanPolicy::default();
    let mut call = plan_call(
        "initial",
        &["filesystem.create_directory", "filesystem.write_text"],
    );
    call.arguments
        .insert("outcome".to_owned(), serde_json::json!("create_file"));
    policy.accept(&call, &all, 12)?;

    policy.record_results(
        &[operation("mkdir", "filesystem.create_directory")],
        &[Ok(completed_operation(
            "mkdir",
            ToolName::FilesystemCreateDirectory,
        ))],
    );
    assert!(!policy.planned_outcome_satisfied());

    policy.record_results(
        &[operation("write", "filesystem.write_text")],
        &[Ok(completed_operation(
            "write",
            ToolName::FilesystemWriteText,
        ))],
    );
    assert!(policy.planned_outcome_satisfied());
    assert!(!policy.outcome_execution_reserve_available);
    Ok(())
}

#[test]
fn adaptive_allowance_is_twice_the_estimate_with_a_hard_cap() -> Result<(), IpcError> {
    let all = [tool(PLAN_TOOL), tool("filesystem.inspect_image")];
    let mut policy = PlanPolicy::default();
    let accepted = policy.accept(
        &plan_call(
            "initial",
            &["filesystem.inspect_image", "filesystem.inspect_image"],
        ),
        &all,
        10,
    )?;
    assert_eq!(accepted["estimatedCalls"], 2);
    assert_eq!(accepted["workingAllowance"], 4);
    assert_eq!(policy.working_allowance(), 4);
    Ok(())
}

#[test]
fn unforecasted_evidence_requires_revision_before_execution() -> Result<(), IpcError> {
    let all = [
        tool(PLAN_TOOL),
        tool("filesystem.inspect_image"),
        tool("filesystem.read_text"),
    ];
    let mut policy = PlanPolicy::default();
    policy.accept(
        &plan_call("initial", &["filesystem.inspect_image"]),
        &all,
        10,
    )?;
    let read = operation("read", "filesystem.read_text");
    let Err(error) = policy.validate_calls(std::slice::from_ref(&read)) else {
        return Err(IpcError::invalid_request(
            "unforecasted evidence read was admitted",
        ));
    };
    assert!(error.message.contains("was not executed"));
    assert!(error.message.contains("filesystem.inspect_image"));
    assert!(error.message.contains("mode=revise"));

    let mut revision = plan_call("revise", &["filesystem.read_text"]);
    revision
        .arguments
        .insert("mode".to_owned(), serde_json::json!("revise"));
    policy.accept(&revision, &all, 8)?;
    assert!(policy.validate_calls(&[read]).is_ok());
    Ok(())
}

#[test]
fn plan_accepts_dependent_discovery_then_exact_loading_with_the_same_tool() -> Result<(), IpcError>
{
    let all = [
        tool(PLAN_TOOL),
        tool("filesystem.collect_evidence"),
        tool("orchestration.transform_artifact"),
    ];
    let mut plan = plan_call("initial", &["orchestration.transform_artifact"]);
    plan.arguments
        .insert("outcome".to_owned(), serde_json::json!("create_file"));
    plan.arguments.insert(
        "steps".to_owned(),
        serde_json::json!([
            {
                "completionEvidence":"Candidate paths returned",
                "expectedCalls":[{
                    "purpose":"Discover source and convention candidates",
                    "providesEvidence":[
                        {"role":"source","evidenceClass":"identity_location"},
                        {"role":"convention","evidenceClass":"identity_location"}
                    ],
                    "tool":"filesystem.collect_evidence"
                }],
                "id":"discover","purpose":"Discover candidate paths",
                "requiredEvidence":["Source and convention locations"]
            },
            {
                "completionEvidence":"Exact source and convention loaded",
                "expectedCalls":[{
                    "purpose":"Load the model-selected exact files",
                    "providesEvidence":[
                        {"role":"source","evidenceClass":"semantic_content"},
                        {"role":"convention","evidenceClass":"format_structure"}
                    ],
                    "tool":"filesystem.collect_evidence"
                }],
                "id":"load","purpose":"Load exact evidence",
                "requiredEvidence":["Source values and convention structure"]
            },
            {
                "completionEvidence":"Artifact verified",
                "expectedCalls":[{"purpose":"Create artifact","tool":"orchestration.transform_artifact"}],
                "id":"write","purpose":"Create artifact","requiredEvidence":["Loaded evidence"]
            }
        ]),
    );

    let accepted = PlanPolicy::default().accept(&plan, &all, 18)?;
    assert_eq!(accepted["estimatedCalls"], 3);
    Ok(())
}

#[test]
fn unscoped_collector_cannot_be_paired_with_redundant_legacy_reads() -> Result<(), IpcError> {
    let all = [
        tool(PLAN_TOOL),
        tool("filesystem.collect_evidence"),
        tool("filesystem.read_text"),
    ];
    let mut plan = plan_call(
        "initial",
        &["filesystem.collect_evidence", "filesystem.read_text"],
    );
    plan.arguments.insert(
        "steps".to_owned(),
        serde_json::json!([{
            "completionEvidence":"Evidence loaded",
            "expectedCalls":[
                {"purpose":"Collect source and convention","tool":"filesystem.collect_evidence"},
                {"purpose":"Read another example","tool":"filesystem.read_text"}
            ],
            "id":"evidence","purpose":"Load evidence","requiredEvidence":["Source and convention"]
        }]),
    );
    let Err(error) = PlanPolicy::default().accept(&plan, &all, 18) else {
        return Err(IpcError::invalid_request(
            "redundant collector and legacy read were accepted",
        ));
    };
    assert!(
        error
            .message
            .contains("both unscoped filesystem.collect_evidence")
    );
    assert!(error.message.contains("earlier step"));

    let mut dependent = plan_call(
        "initial",
        &["filesystem.read_text", "filesystem.collect_evidence"],
    );
    dependent.arguments.insert(
        "steps".to_owned(),
        serde_json::json!([
            {
                "completionEvidence":"Index loaded",
                "expectedCalls":[{"purpose":"Orient from an index","tool":"filesystem.read_text"}],
                "id":"orient","purpose":"Resolve the dependent target","requiredEvidence":["Index mapping"]
            },
            {
                "completionEvidence":"Dependent evidence loaded",
                "expectedCalls":[{"purpose":"Load the resolved evidence","tool":"filesystem.collect_evidence"}],
                "id":"load","purpose":"Load dependent evidence","requiredEvidence":["Resolved target"]
            }
        ]),
    );
    PlanPolicy::default().accept(&dependent, &all, 18)?;
    Ok(())
}

#[test]
fn exhausted_discovery_allowance_preserves_one_planned_mutation() -> Result<(), IpcError> {
    let all = [
        tool(PLAN_TOOL),
        tool("filesystem.read_text"),
        tool("filesystem.replace_text"),
    ];
    let mut call = plan_call(
        "initial",
        &["filesystem.read_text", "filesystem.replace_text"],
    );
    call.arguments
        .insert("outcome".to_owned(), serde_json::json!("modify_file"));
    let mut policy = PlanPolicy::default();
    policy.accept(&call, &all, 10)?;
    policy.operations_used = policy.working_allowance;

    assert!(
        policy
            .validate_calls(&[operation("extra-read", "filesystem.read_text")])
            .is_err()
    );
    assert!(
        policy
            .validate_calls(&[operation("edit", "filesystem.replace_text")])
            .is_ok()
    );
    assert!(
        policy
            .validate_calls(&[operation("second-edit", "filesystem.replace_text")])
            .is_err()
    );
    Ok(())
}

#[test]
fn revisions_preserve_consumed_work_and_refresh_the_remaining_estimate() -> Result<(), IpcError> {
    let all = [tool(PLAN_TOOL), tool("filesystem.inspect_image")];
    let mut policy = PlanPolicy::default();
    policy.accept(
        &plan_call("initial", &["filesystem.inspect_image"]),
        &all,
        10,
    )?;
    policy.operations_used = 1;
    let revised = policy.accept(&plan_call("revise", &["filesystem.inspect_image"]), &all, 8)?;
    assert_eq!(policy.operations_used(), 1);
    assert_eq!(revised["workingAllowance"], 3);
    assert_eq!(policy.working_allowance(), 4);

    let mutation_tools = [tool(PLAN_TOOL), tool("filesystem.write_text")];
    let mut create = plan_call("initial", &["filesystem.write_text"]);
    create
        .arguments
        .insert("outcome".to_owned(), serde_json::json!("create_file"));
    let mut mutation_policy = PlanPolicy::default();
    mutation_policy.accept(&create, &mutation_tools, 8)?;
    assert!(mutation_policy.outcome_execution_reserve_available);
    mutation_policy.forecast_steps[0].blocked = true;
    create
        .arguments
        .insert("mode".to_owned(), serde_json::json!("revise"));
    mutation_policy.accept(&create, &mutation_tools, 7)?;
    assert!(mutation_policy.outcome_execution_reserve_available);
    mutation_policy.outcome_execution_reserve_available = false;
    mutation_policy.forecast_steps[0].blocked = true;
    mutation_policy.accept(&create, &mutation_tools, 6)?;
    assert!(!mutation_policy.outcome_execution_reserve_available);
    Ok(())
}

#[test]
fn unknown_predicted_tools_and_excessive_revisions_are_rejected() -> Result<(), IpcError> {
    let all = [tool(PLAN_TOOL), tool("filesystem.inspect_image")];
    let mut policy = PlanPolicy::default();
    assert!(
        policy
            .accept(&plan_call("initial", &["filesystem.not_real"]), &all, 10,)
            .is_err()
    );
    policy.accept(
        &plan_call("initial", &["filesystem.inspect_image"]),
        &all,
        10,
    )?;
    for _ in 0..MAX_PLAN_REVISIONS {
        policy.accept(
            &plan_call("revise", &["filesystem.inspect_image"]),
            &all,
            10,
        )?;
    }
    assert!(
        policy
            .accept(
                &plan_call("revise", &["filesystem.inspect_image"]),
                &all,
                10,
            )
            .is_err()
    );
    Ok(())
}

#[test]
fn plan_contract_repairs_allow_three_identical_attempts_and_remain_bounded() {
    let mut policy = PlanPolicy::default();
    let first = policy_error("first malformed contract");
    assert!(policy.correction_available(&first));
    policy.record_correction(&first);
    assert!(policy.correction_pending());
    assert!(policy.correction_available(&first));
    policy.record_correction(&first);
    assert!(policy.correction_available(&first));
    policy.record_correction(&first);
    assert!(!policy.correction_available(&first));

    let second = policy_error("second malformed contract");
    let third = policy_error("third malformed contract");
    let fourth = policy_error("fourth malformed contract");
    assert!(policy.correction_available(&second));
    policy.record_correction(&second);
    assert!(policy.correction_available(&third));
    policy.record_correction(&third);
    assert!(policy.correction_available(&fourth));
    policy.record_correction(&fourth);
    assert!(!policy.correction_available(&fourth));
}

#[test]
fn orchestration_controls_are_rejected_from_the_exact_nested_forecast_location()
-> Result<(), IpcError> {
    let all = [tool(PLAN_TOOL), tool("computation.run_project")];
    let mut call = plan_call("initial", &[PLAN_TOOL]);
    call.arguments.insert(
        "steps".to_owned(),
        serde_json::json!([{
            "completionEvidence": "The answer is presented",
            "expectedCalls": [{
                "purpose": "Report the result",
                "tool": PLAN_TOOL
            }],
            "id": "report",
            "purpose": "Present the final answer",
            "requiredEvidence": ["Completed computation"]
        }]),
    );

    let Err(error) = PlanPolicy::default().accept(&call, &all, 10) else {
        return Err(IpcError::internal());
    };

    assert_eq!(error.code, IpcErrorCode::InvalidRequest);
    assert!(error.retryable);
    assert!(error.message.contains("Plan step `report`"));
    assert!(error.message.contains("`orchestration.submit_plan`"));
    assert!(error.message.contains("steps[0].expectedCalls[0].tool"));
    assert!(error.message.contains("Remove this entire step"));
    assert!(error.message.contains("do not invent an execution call"));
    Ok(())
}

#[test]
fn answer_forecast_ends_at_the_last_evidence_tool_without_a_reporting_call() -> Result<(), IpcError>
{
    let all = [
        tool(PLAN_TOOL),
        tool("filesystem.read_text"),
        tool("computation.run_project"),
    ];
    let mut call = plan_call(
        "initial",
        &["filesystem.read_text", "computation.run_project"],
    );
    call.arguments
        .insert("outcome".to_owned(), serde_json::json!("answer"));
    let mut policy = PlanPolicy::default();
    let accepted = policy.accept(&call, &all, 10)?;
    assert_eq!(accepted["estimatedCalls"], 2);

    policy.record_results(
        &[
            operation("read", "filesystem.read_text"),
            operation("compute", "computation.run_project"),
        ],
        &[
            Ok(completed_operation("read", ToolName::FilesystemReadText)),
            Ok(completed_operation(
                "compute",
                ToolName::ComputationRunProject,
            )),
        ],
    );

    assert_eq!(
        policy.progress_projection(false),
        Some(ToolPlanProgress::new(
            1,
            0,
            vec![
                ToolPlanStepProgress::new(1, ToolPlanStepStatus::ForecastComplete, Vec::new(),)
                    .with_contract(
                        "produce".to_owned(),
                        "Inspect evidence and produce the artifact".to_owned(),
                        vec!["Source and convention evidence".to_owned()],
                        "The requested artifact is verified".to_owned(),
                        vec!["read".to_owned(), "compute".to_owned()],
                    )
            ],
            None,
            ToolPlanOutcome::Answer,
            false,
            false,
            2,
            false,
            0,
        ))
    );
    assert!(!policy.expects_file_mutation());
    Ok(())
}
