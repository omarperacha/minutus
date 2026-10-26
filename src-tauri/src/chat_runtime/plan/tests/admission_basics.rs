use super::*;

#[test]
fn execution_ready_scope_exposes_only_the_outcome_operation() -> Result<(), IpcError> {
    let all = [
        tool(PLAN_TOOL),
        tool(FORECAST_TOOL),
        tool("filesystem.inspect_image"),
        tool("filesystem.read_text"),
        tool("filesystem.write_text"),
    ];
    let mut policy = PlanPolicy::default();
    assert_eq!(policy.tools(&all).len(), 1);
    assert_eq!(policy.tools(&all)[0].name, PLAN_TOOL);
    let mut plan = plan_call("initial", &["filesystem.write_text"]);
    plan.arguments
        .insert("outcome".to_owned(), serde_json::json!("create_file"));
    policy.accept(&plan, &all, 12)?;
    assert_eq!(
        policy
            .tools(&all)
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        ["orchestration.submit_plan", "filesystem.write_text"]
    );

    let write = operation("write", "filesystem.write_text");
    policy.record_results(
        std::slice::from_ref(&write),
        &[Ok(failed_operation("write", ToolName::FilesystemWriteText))],
    );
    assert_eq!(policy.tools(&all).len(), 2);
    Ok(())
}

#[test]
fn execution_ready_scope_keeps_a_bounded_revision_escape() -> Result<(), IpcError> {
    let all = [
        tool(PLAN_TOOL),
        tool("filesystem.collect_evidence"),
        tool("filesystem.write_text"),
    ];
    let mut initial = plan_call("initial", &["filesystem.write_text"]);
    initial
        .arguments
        .insert("outcome".to_owned(), serde_json::json!("create_file"));
    let mut revision = initial.clone();
    revision.call_id = "plan-revise".to_owned();
    revision
        .arguments
        .insert("mode".to_owned(), serde_json::json!("revise"));

    let mut policy = PlanPolicy::default();
    policy.accept(&initial, &all, 12)?;
    assert!(policy.revision_available());
    policy.accept(&revision, &all, 10)?;
    assert_eq!(policy.revisions_used, 1);
    Ok(())
}

#[test]
fn bounded_stage_requires_exact_host_proven_readiness_and_verified_postcondition()
-> Result<(), IpcError> {
    let all = [
        tool(PLAN_TOOL),
        tool(FORECAST_TOOL),
        tool("filesystem.find_paths"),
        tool("filesystem.read_text"),
        tool("computation.run"),
        tool("filesystem.write_text"),
    ];
    let mut policy = PlanPolicy::default();
    policy.accept(&exact_bounded_write_plan(), &all, 8)?;

    assert!(policy.bounded_stage_contract(&all, true).is_none());
    let mut locate = operation("location-receipt", "filesystem.find_paths");
    locate.arguments.insert(
        "queries".to_owned(),
        serde_json::json!([{"nameContains":"source"}]),
    );
    policy.record_results(
        std::slice::from_ref(&locate),
        &[Ok(completed_find_paths_operation(
            "location-receipt",
            &[(1, true)],
        ))],
    );
    let read_contract = policy
        .bounded_stage_contract(&all, true)
        .ok_or_else(IpcError::internal)?;
    assert_eq!(read_contract.operation(), "filesystem.read_text");

    let read = operation("source-receipt", "filesystem.read_text");
    policy.record_results(
        std::slice::from_ref(&read),
        &[Ok(completed_operation(
            "source-receipt",
            ToolName::FilesystemReadText,
        ))],
    );
    let computation_contract = policy
        .bounded_stage_contract(&all, true)
        .ok_or_else(IpcError::internal)?;
    assert_eq!(computation_contract.operation(), "computation.run");
    let compute = operation("computation-receipt", "computation.run");
    policy.record_results_with_stage_contract(
        std::slice::from_ref(&compute),
        &[Ok(completed_operation(
            "computation-receipt",
            ToolName::ComputationRun,
        ))],
        Some(&computation_contract),
    );
    assert!(policy.bounded_stage_contract(&all, false).is_none());
    let write_contract = policy
        .bounded_stage_contract(&all, true)
        .ok_or_else(IpcError::internal)?;
    assert_eq!(write_contract.operation(), "filesystem.write_text");
    assert_eq!(
        write_contract.evidence_call_ids,
        vec!["computation-receipt".to_owned()]
    );

    let write = operation("write-attempt", "filesystem.write_text");
    let mut unverified = completed_operation("write-attempt", ToolName::FilesystemWriteText);
    if let ToolOutcome::Completed { content } = &mut unverified.result.outcome
        && let Some(ToolResultPart::Json {
            value: JsonValue::Object(fields),
        }) = content.first_mut()
        && let Some(JsonValue::Array(changes)) = fields.get_mut("changes")
        && let Some(JsonValue::Object(change)) = changes.first_mut()
    {
        change.insert("postWriteVerified".to_owned(), JsonValue::Bool(false));
    }
    policy.record_results_with_stage_contract(
        std::slice::from_ref(&write),
        &[Ok(unverified)],
        Some(&write_contract),
    );
    assert!(!policy.planned_outcome_satisfied());
    assert!(
        policy
            .forecast_steps
            .iter()
            .find(|step| step.id == "write")
            .is_some_and(ForecastStep::unfinished)
    );
    Ok(())
}

#[test]
fn bounded_stage_postconditions_cover_generic_text_table_computation_and_file_work() {
    assert_eq!(
        stage_postcondition("filesystem.read_text"),
        Some(ToolStagePostcondition::TextLoaded)
    );
    assert_eq!(
        stage_postcondition("filesystem.write_delimited_table"),
        Some(ToolStagePostcondition::FileWriteVerified)
    );
    assert_eq!(
        stage_postcondition("computation.run_project"),
        Some(ToolStagePostcondition::ComputationCompleted)
    );
    assert_eq!(
        stage_postcondition("filesystem.create_directory"),
        Some(ToolStagePostcondition::DirectoryCreateVerified)
    );
    assert_eq!(stage_postcondition("filesystem.scan"), None);
}

#[test]
fn semantic_table_stage_binds_exact_dependency_receipts_and_qualifies() -> Result<(), IpcError> {
    let all = [
        tool(PLAN_TOOL),
        tool("filesystem.inspect_image"),
        tool("filesystem.read_text"),
        tool("filesystem.write_delimited_table"),
    ];
    let mut plan = semantic_plan_call("initial");
    plan.arguments.insert(
        "steps".to_owned(),
        serde_json::json!([
            {
                "actionClass":"read", "dependsOn":[],
                "doneWhen":"Source loaded", "id":"source",
                "objective":"Load source", "preferredTools":["filesystem.inspect_image"],
                "requiredEvidence":["semantic_content"]
            },
            {
                "actionClass":"read", "dependsOn":[],
                "doneWhen":"Convention loaded", "id":"convention",
                "objective":"Load convention", "preferredTools":["filesystem.read_text"],
                "requiredEvidence":["format_structure"]
            },
            {
                "actionClass":"workspace_write", "dependsOn":["source","convention"],
                "doneWhen":"Artifact verified", "id":"artifact",
                "objective":"Materialize table", "preferredTools":["filesystem.write_delimited_table"],
                "requiredEvidence":["semantic_content","format_structure"]
            }
        ]),
    );
    let mut policy = PlanPolicy::default();
    policy.accept(&plan, &all, 8)?;
    let source = operation("source-call", "filesystem.inspect_image");
    let convention = operation("convention-call", "filesystem.read_text");
    policy.record_results(
        &[source, convention],
        &[
            Ok(completed_operation(
                "source-call",
                ToolName::FilesystemInspectImage,
            )),
            Ok(completed_operation(
                "convention-call",
                ToolName::FilesystemReadText,
            )),
        ],
    );

    let contract = policy
        .bounded_stage_contract(&all, true)
        .ok_or_else(IpcError::internal)?;
    assert_eq!(contract.operation(), "filesystem.write_delimited_table");
    assert_eq!(
        contract.evidence_call_ids,
        vec!["source-call".to_owned(), "convention-call".to_owned()]
    );
    let mut artifact = operation("artifact-call", "filesystem.write_delimited_table");
    artifact.arguments.insert(
        "evidence".to_owned(),
        serde_json::json!([
            {"callId":"invented","role":"source"},
            {"callId":"invented","role":"convention"}
        ]),
    );
    policy.bind_artifact_evidence(std::slice::from_mut(&mut artifact));
    assert_eq!(
        artifact.arguments["evidence"],
        serde_json::json!([
            {"callId":"source-call","role":"source"},
            {"callId":"convention-call","role":"convention"}
        ])
    );
    policy.validate_artifact_evidence_bindings(&artifact)?;
    Ok(())
}

#[test]
fn completed_answer_forecast_closes_tools_and_requires_the_final_response() -> Result<(), IpcError>
{
    let all = [
        tool(PLAN_TOOL),
        tool("filesystem.read_text"),
        tool("filesystem.find_paths"),
    ];
    let mut initial = plan_call("initial", &["filesystem.read_text"]);
    initial
        .arguments
        .insert("outcome".to_owned(), serde_json::json!("answer"));
    let mut policy = PlanPolicy::default();
    policy.accept(&initial, &all, 8)?;
    assert_eq!(policy.tools(&all).len(), 2);

    let read = operation("read", "filesystem.read_text");
    policy.record_results(
        std::slice::from_ref(&read),
        &[Ok(completed_operation(
            "read",
            ToolName::FilesystemReadText,
        ))],
    );
    assert!(policy.final_response_ready());
    assert!(policy.tools(&all).is_empty());
    assert!(!policy.revision_available());

    let mut revision = initial;
    revision.call_id = "plan-revise".to_owned();
    revision
        .arguments
        .insert("mode".to_owned(), serde_json::json!("revise"));
    let error = policy
        .accept(&revision, &all, 7)
        .err()
        .ok_or_else(IpcError::internal)?;
    assert!(error.message.contains("final response"));
    Ok(())
}

#[test]
fn legacy_first_tool_batch_may_include_initial_operations() {
    let plan = plan_call("initial", &["filesystem.read_text"]);
    let read = operation("read", "filesystem.read_text");
    let mut policy = PlanPolicy::default();

    assert!(policy.validate_calls(std::slice::from_ref(&read)).is_err());
    assert!(policy.validate_calls(&[plan.clone(), read.clone()]).is_ok());
    assert!(policy.validate_calls(&[read, plan]).is_err());
}

#[test]
fn plan_mode_schema_tracks_host_state_and_stale_initial_is_normalized() -> Result<(), IpcError> {
    let all = [
        tool_with_parameters(
            PLAN_TOOL,
            &serde_json::json!({"mode":{"type":"string","enum":["initial","revise"]}}),
        ),
        tool("filesystem.read_text"),
    ];
    let mut policy = PlanPolicy::default();
    let initial_tools = policy.tools(&all);
    let initial_plan = initial_tools
        .iter()
        .find(|tool| tool.name == PLAN_TOOL)
        .ok_or_else(IpcError::internal)?;
    assert_eq!(
        initial_plan.parameters["properties"]["mode"]["enum"],
        serde_json::json!(["initial"])
    );

    let original = plan_with_step_count(1);
    policy.accept(&original, &all, 8)?;
    let active_tools = policy.tools(&all);
    let revision_plan = active_tools
        .iter()
        .find(|tool| tool.name == PLAN_TOOL)
        .ok_or_else(IpcError::internal)?;
    assert_eq!(
        revision_plan.parameters["properties"]["mode"]["enum"],
        serde_json::json!(["revise"])
    );
    assert!(
        revision_plan
            .description
            .starts_with("Optional recovery control.")
    );

    let mut redundant = original.clone();
    redundant.call_id = "redundant".to_owned();
    assert!(policy.redundant_stale_initial(&redundant));
    assert!(!policy.normalize_stale_initial(&mut redundant));
    assert_eq!(policy.redundant_plan_value()["planUnchanged"], true);

    let mut changed = original;
    changed.call_id = "changed".to_owned();
    changed
        .arguments
        .insert("target".to_owned(), serde_json::json!("A changed target"));
    assert!(policy.normalize_stale_initial(&mut changed));
    assert_eq!(changed.arguments["mode"], "revise");
    Ok(())
}

#[test]
fn staged_external_tool_is_visible_before_plan_but_cannot_execute() -> Result<(), IpcError> {
    let all = [tool(PLAN_TOOL), tool("web.search"), tool("x402.request")];
    let mut policy = PlanPolicy::default();
    let initial_names = policy
        .tools(&all)
        .into_iter()
        .map(|tool| tool.name)
        .collect::<BTreeSet<_>>();
    assert!(initial_names.contains("x402.request"));

    let external = operation("purchase", "x402.request");
    let Err(error) = policy.validate_calls(std::slice::from_ref(&external)) else {
        return Err(IpcError::invalid_request(
            "a visible staged effect executed before plan admission",
        ));
    };
    assert!(error.message.contains("was not executed"));
    assert!(error.message.contains("plan by itself"));
    assert!(!policy.active());
    Ok(())
}
