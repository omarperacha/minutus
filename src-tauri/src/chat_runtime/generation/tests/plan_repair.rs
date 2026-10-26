use super::*;

#[test]
fn layered_v6_plan_defects_receive_bounded_repairs_before_admission() -> Result<(), IpcError> {
    let tools = [
        "orchestration.submit_plan",
        "filesystem.inspect_image",
        "filesystem.write_text",
    ]
    .into_iter()
    .map(prompt_tool)
    .collect::<Vec<_>>();
    let valid = v6_file_plan();
    let mut stringified = valid.clone();
    let steps = stringified
        .arguments
        .get("steps")
        .cloned()
        .ok_or_else(IpcError::internal)?;
    stringified.arguments.insert(
        "steps".to_owned(),
        serde_json::json!(serde_json::to_string(&steps).map_err(|_| IpcError::internal())?),
    );
    let mut unavailable = valid.clone();
    let unavailable_tools = unavailable
        .arguments
        .get_mut("steps")
        .and_then(serde_json::Value::as_array_mut)
        .and_then(|steps| steps.get_mut(1))
        .and_then(|step| step.get_mut("suggestedTools"))
        .ok_or_else(IpcError::internal)?;
    *unavailable_tools = serde_json::json!(["filesystem.unavailable"]);

    let mut state = GenerationToolState::default();
    for invalid in [&stringified, &unavailable] {
        let error = state
            .plan_policy
            .accept(invalid, &tools, 8)
            .err()
            .ok_or_else(IpcError::internal)?;
        assert!(state.plan_policy.correction_available(&error));
        assert!(
            state
                .request_plan_recovery(
                    state.plan_policy.correction_scope(),
                    plan_repair_defects(&error).ok_or_else(IpcError::internal)?,
                    error.message.clone(),
                )
                .is_ok()
        );
        state.plan_policy.record_correction(&error);
    }

    state.plan_policy.accept(&valid, &tools, 8)?;
    assert_eq!(state.plan_policy.revision(), 1);
    Ok(())
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "the regression keeps the complete plan-plus-malformed-operation trajectory visible"
)]
fn golden_valid_plan_plus_one_malformed_operation_survives_bounded_repairs() -> Result<(), IpcError>
{
    let plan = ParsedToolCall {
        call_id: "plan".to_owned(),
        recipient: "orchestration.submit_plan".to_owned(),
        name: "orchestration.submit_plan".to_owned(),
        arguments: BTreeMap::from([
            ("assumptions".to_owned(), serde_json::json!([])),
            ("mode".to_owned(), serde_json::json!("initial")),
            ("outcome".to_owned(), serde_json::json!("create_file")),
            (
                "steps".to_owned(),
                serde_json::json!([
                    {
                        "completionEvidence": "Source and convention evidence loaded",
                        "expectedCalls": [{
                            "purpose":"Collect evidence",
                            "providesEvidence":[
                                {"role":"source","evidenceClass":"semantic_content"},
                                {"role":"convention","evidenceClass":"format_structure"}
                            ],
                            "tool":"filesystem.collect_evidence"
                        }],
                        "id": "locate",
                        "purpose": "Collect source and convention evidence",
                        "requiredEvidence": ["Semantic source image", "Format and storage manifest"]
                    },
                    {
                        "completionEvidence": "Verified artifact created",
                        "expectedCalls": [{"purpose":"Create CSV","tool":"orchestration.transform_artifact"}],
                        "id": "write",
                        "purpose": "Create the requested CSV",
                        "requiredEvidence": ["Transcribed semantic source", "Manifest-derived CSV convention"]
                    }
                ]),
            ),
            (
                "successCriteria".to_owned(),
                serde_json::json!(["The requested CSV is verified"]),
            ),
            (
                "target".to_owned(),
                serde_json::json!(
                    "outputs/reconstructed_tables/monthly cashflow/monthly cashflow.csv"
                ),
            ),
        ]),
    };
    let malformed = ParsedToolCall {
        call_id: "collect".to_owned(),
        recipient: "filesystem.collect_evidence".to_owned(),
        name: "filesystem.collect_evidence".to_owned(),
        arguments: BTreeMap::from([
            (
                "items".to_owned(),
                serde_json::json!([
                    {"id":"all_csvs","extensions":["csv"],"requirement":"Locate reconstructed tables","operation":"find_matches"},
                    {"id":"monthly_dir","path":"monthly cashflow","scope":"directory","requirement":"Locate monthly source files","operation":"find_matches"}
                ]),
            ),
            ("workspace".to_owned(), serde_json::json!("primary")),
        ]),
    };
    let tools = [
        "orchestration.submit_plan",
        "filesystem.collect_evidence",
        "orchestration.transform_artifact",
    ]
    .into_iter()
    .map(|name| PromptToolDefinition {
        name: name.to_owned(),
        description: String::new(),
        parameters: serde_json::json!({}),
    })
    .collect::<Vec<_>>();
    let calls = vec![plan.clone(), malformed];
    let mut repair_state = GenerationToolState::default();

    let error = IpcError::invalid_request("items must be an array");
    assert!(valid_plan_prefixed_operation_repair_allowed(
        &repair_state,
        &calls,
        1,
        &tools,
        &error
    ));
    record_argument_correction(&mut repair_state, std::slice::from_ref(&calls[1]), &error);
    assert!(valid_plan_prefixed_operation_repair_allowed(
        &repair_state,
        &calls,
        1,
        &tools,
        &error
    ));
    record_argument_correction(&mut repair_state, std::slice::from_ref(&calls[1]), &error);
    record_argument_correction(&mut repair_state, std::slice::from_ref(&calls[1]), &error);
    assert!(valid_plan_prefixed_operation_repair_allowed(
        &repair_state,
        &calls,
        1,
        &tools,
        &error
    ));
    repair_state.argument_corrections = MAX_ARGUMENT_CORRECTIONS;
    assert!(!valid_plan_prefixed_operation_repair_allowed(
        &repair_state,
        &calls,
        1,
        &tools,
        &error
    ));

    let mut state = GenerationToolState::default();
    assert_eq!(state.plan_policy.revision(), 0);
    let remaining = remaining_plan_forecast_calls(state.total_calls, calls.len());
    state.plan_policy.accept(&plan, &tools, remaining)?;
    assert_eq!(state.plan_policy.revision(), 1);
    assert_eq!(
        state.plan_policy.current_step_unfinished_tools(),
        vec!["filesystem.collect_evidence"]
    );

    Ok(())
}

#[test]
fn revision_prefixed_operation_is_checked_against_the_revised_scope() -> Result<(), IpcError> {
    let plan = |mode: &str, tool: &str, call_id: &str| ParsedToolCall {
        call_id: call_id.to_owned(),
        recipient: "orchestration.submit_plan".to_owned(),
        name: "orchestration.submit_plan".to_owned(),
        arguments: BTreeMap::from([
            ("assumptions".to_owned(), serde_json::json!([])),
            ("mode".to_owned(), serde_json::json!(mode)),
            ("outcome".to_owned(), serde_json::json!("answer")),
            (
                "steps".to_owned(),
                serde_json::json!([{
                    "completionEvidence":"The advertised operation completed",
                    "expectedCalls":[{"purpose":"Resolve the current dependency","tool":tool}],
                    "id":"resolve",
                    "purpose":"Resolve the current dependency",
                    "requiredEvidence":["The operation result"]
                }]),
            ),
            (
                "successCriteria".to_owned(),
                serde_json::json!(["The requested answer is supported"]),
            ),
            ("target".to_owned(), serde_json::json!("Answer the request")),
        ]),
    };
    let all_tools = [
        "orchestration.submit_plan",
        "filesystem.stat",
        "filesystem.collect_evidence",
    ]
    .into_iter()
    .map(|name| PromptToolDefinition {
        name: name.to_owned(),
        description: String::new(),
        parameters: serde_json::json!({}),
    })
    .collect::<Vec<_>>();
    let mut state = GenerationToolState::default();
    state.plan_policy.accept(
        &plan("initial", "filesystem.stat", "initial"),
        &all_tools,
        8,
    )?;
    let current_tools = state.plan_policy.tools(&all_tools);
    assert!(
        current_tools
            .iter()
            .all(|tool| tool.name != "filesystem.collect_evidence")
    );

    let revision = plan("revise", "filesystem.collect_evidence", "revision");
    let malformed = ParsedToolCall {
        call_id: "collect".to_owned(),
        recipient: "filesystem.collect_evidence".to_owned(),
        name: "filesystem.collect_evidence".to_owned(),
        arguments: BTreeMap::from([
            ("items".to_owned(), serde_json::json!("not-an-array")),
            ("workspace".to_owned(), serde_json::json!("primary")),
        ]),
    };
    let calls = vec![revision, malformed];
    let prospective = prospective_plan_tools(&current_tools, &all_tools, &state, &calls);
    assert!(
        prospective
            .iter()
            .any(|tool| tool.name == "filesystem.collect_evidence")
    );
    assert!(valid_plan_prefixed_operation_repair_allowed(
        &state,
        &calls,
        1,
        &all_tools,
        &IpcError::invalid_request("items must be an array"),
    ));
    assert_eq!(state.plan_policy.revision(), 1);
    Ok(())
}

#[test]
fn paraphrasing_a_stringified_plan_does_not_count_as_structural_repair() -> Result<(), IpcError> {
    let tools = [
        "orchestration.submit_plan",
        "filesystem.inspect_image",
        "filesystem.write_text",
    ]
    .into_iter()
    .map(prompt_tool)
    .collect::<Vec<_>>();
    let mut previous = None;
    for text in [
        "[]".to_owned(),
        "{}".to_owned(),
        "x".repeat(300),
        "x".repeat(310),
    ] {
        let mut plan = v6_file_plan();
        plan.arguments
            .insert("steps".into(), serde_json::json!(text));
        let mut state = GenerationToolState::default();
        let error = state
            .plan_policy
            .accept(&plan, &tools, 8)
            .err()
            .ok_or_else(IpcError::internal)?;
        let defects = plan_repair_defects(&error).ok_or_else(IpcError::internal)?;
        if let Some(prior) = &previous {
            assert_eq!(prior, &defects);
        }
        previous = Some(defects);
    }
    Ok(())
}
