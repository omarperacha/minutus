use super::*;

#[test]
fn scoped_same_tool_calls_require_the_expected_target_and_recover_deliberately()
-> Result<(), IpcError> {
    let all = [
        tool(PLAN_TOOL),
        tool_with_parameters(
            "filesystem.read_text",
            &serde_json::json!({"path":{"type":"string"}}),
        ),
        tool("filesystem.find_paths"),
    ];
    let mut plan = plan_with_step_count(2);
    plan.arguments.insert(
        "steps".to_owned(),
        serde_json::json!([
            {
                "completionEvidence":"Source loaded",
                "expectedCalls":[{"purpose":"Read source","scope":{"argument":"path","value":"source.txt"},"tool":"filesystem.read_text"}],
                "id":"source","purpose":"Load source","requiredEvidence":["Exact source"]
            },
            {
                "completionEvidence":"Convention loaded",
                "expectedCalls":[{"purpose":"Read convention","scope":{"argument":"path","value":"manifest.json"},"tool":"filesystem.read_text"}],
                "id":"convention","purpose":"Load convention","requiredEvidence":["Exact convention"]
            }
        ]),
    );
    let mut policy = PlanPolicy::default();
    policy.accept(&plan, &all, 18)?;

    let later = scoped_operation("later", "filesystem.read_text", "path", "manifest.json");
    assert_eq!(
        policy.diagnostic_match(&later),
        (1, Some(2), ForecastDiagnosticMatch::OutOfOrder)
    );
    policy.record_results(
        std::slice::from_ref(&later),
        &[Ok(completed_operation(
            "later",
            ToolName::FilesystemReadText,
        ))],
    );
    assert!(policy.deviation_pending());
    assert_eq!(policy.unfinished_call_count(), 2);

    let unrelated = scoped_operation("confidence", "filesystem.find_paths", "path", ".");
    policy.record_results(
        &[unrelated],
        &[Ok(completed_operation(
            "confidence",
            ToolName::FilesystemFindPaths,
        ))],
    );
    assert!(policy.deviation_pending());
    assert_eq!(policy.unforecasted_operations, 2);

    let source = scoped_operation("source", "filesystem.read_text", "path", "source.txt");
    policy.record_results(
        std::slice::from_ref(&source),
        &[Ok(failed_operation("source", ToolName::FilesystemReadText))],
    );
    assert!(!policy.deviation_pending());
    assert!(
        policy.forecast_steps[0].blocked,
        "returning to the forecast is deliberate even when evidence fails"
    );

    let mut revision = plan_with_step_count(1);
    revision
        .arguments
        .insert("mode".to_owned(), serde_json::json!("revise"));
    revision.arguments.insert(
        "steps".to_owned(),
        serde_json::json!([{
            "completionEvidence":"The missing source is resolved",
            "expectedCalls":[{"purpose":"Resolve exact missing source","tool":"filesystem.find_paths"}],
            "id":"refine","purpose":"Resolve the evidence gap",
            "requiredEvidence":["Concrete missing source path"]
        }]),
    );
    let operations_before = policy.operations_used();
    policy.accept(&revision, &all, 12)?;
    assert!(!policy.deviation_pending());
    assert_eq!(policy.operations_used(), operations_before);
    assert!(!policy.outcome_execution_reserve_available);
    Ok(())
}

#[test]
fn one_batched_operation_satisfies_each_scoped_forecast_present_in_its_arguments()
-> Result<(), IpcError> {
    let all = [
        tool(PLAN_TOOL),
        tool_with_parameters(
            "filesystem.find_paths",
            &serde_json::json!({
                "queries":{
                    "type":"array",
                    "items":{
                        "type":"object",
                        "properties":{"nameContains":{"type":"string"}}
                    }
                }
            }),
        ),
    ];
    let mut plan = plan_with_step_count(1);
    plan.arguments.insert(
        "steps".to_owned(),
        serde_json::json!([{
            "completionEvidence":"Both filename searches completed",
            "expectedCalls":[
                {"purpose":"Find the source","scope":{"argument":"queries","value":"cashflow"},"tool":"filesystem.find_paths"},
                {"purpose":"Find convention examples","scope":{"argument":"queries","value":"reconstructed"},"tool":"filesystem.find_paths"}
            ],
            "id":"discover",
            "purpose":"Locate source and convention files",
            "requiredEvidence":["Source path","Convention path"]
        }]),
    );
    let mut policy = PlanPolicy::default();
    policy.accept(&plan, &all, 18)?;

    let mut batched = operation("discover", "filesystem.find_paths");
    batched.arguments.insert(
        "queries".to_owned(),
        serde_json::json!([
            {"extensions":["png"],"nameContains":"cashflow"},
            {"extensions":["csv"],"nameContains":"reconstructed"}
        ]),
    );
    assert_eq!(
        policy.diagnostic_match(&batched),
        (1, Some(1), ForecastDiagnosticMatch::Matched)
    );
    policy.record_results(
        std::slice::from_ref(&batched),
        &[Ok(completed_find_paths_operation(
            "discover",
            &[(1, false), (1, false)],
        ))],
    );

    assert!(policy.forecast_complete());
    assert_eq!(policy.unfinished_call_count(), 0);
    assert_eq!(policy.unforecasted_operations, 0);
    Ok(())
}

#[test]
fn incomplete_discovery_keeps_the_same_step_and_tool_refinable() -> Result<(), IpcError> {
    let all = [tool(PLAN_TOOL), tool("filesystem.find_paths")];
    let mut plan = plan_with_step_count(1);
    plan.arguments.insert(
        "steps".to_owned(),
        serde_json::json!([{
            "completionEvidence":"Both required locations are resolved",
            "expectedCalls":[{"purpose":"Locate both inputs","tool":"filesystem.find_paths"}],
            "id":"discover",
            "purpose":"Resolve the source and convention locations",
            "requiredEvidence":["Source path","Convention path"]
        }]),
    );
    let mut policy = PlanPolicy::default();
    policy.accept(&plan, &all, 18)?;
    let mut discovery = operation("discover", "filesystem.find_paths");
    discovery.arguments.insert(
        "queries".to_owned(),
        serde_json::json!([
            {"nameContains":"input","extensions":["png"]},
            {"nameContains":"example","extensions":["csv"]}
        ]),
    );

    policy.record_results(
        std::slice::from_ref(&discovery),
        &[Ok(completed_find_paths_operation(
            "discover",
            &[(1, false), (0, false)],
        ))],
    );

    assert!(!policy.forecast_complete());
    assert_eq!(
        policy.current_step_unfinished_tools(),
        vec!["filesystem.find_paths"]
    );
    assert!(policy.forecast_steps[0].blocked);
    assert!(
        policy
            .tools(&all)
            .iter()
            .any(|tool| tool.name == "filesystem.find_paths")
    );
    Ok(())
}

#[test]
fn exhaustive_empty_discovery_can_prove_a_selector_resolved() -> Result<(), IpcError> {
    let all = [tool(PLAN_TOOL), tool("filesystem.find_paths")];
    let mut policy = PlanPolicy::default();
    policy.accept(&plan_call("initial", &["filesystem.find_paths"]), &all, 18)?;
    let mut discovery = operation("discover", "filesystem.find_paths");
    discovery.arguments.insert(
        "queries".to_owned(),
        serde_json::json!([{"nameContains":"optional-marker"}]),
    );
    policy.record_results(
        std::slice::from_ref(&discovery),
        &[Ok(completed_find_paths_operation("discover", &[(0, true)]))],
    );
    assert!(policy.forecast_complete());
    Ok(())
}

#[test]
fn safe_read_recovery_reopens_prior_work_or_grants_a_bounded_escape() -> Result<(), IpcError> {
    let all = [
        tool(PLAN_TOOL),
        tool("filesystem.find_paths"),
        tool("filesystem.inspect_image"),
        tool("filesystem.read_text"),
    ];
    let mut plan = plan_with_step_count(2);
    plan.arguments.insert(
        "steps".to_owned(),
        serde_json::json!([
            {
                "completionEvidence":"Location resolved",
                "expectedCalls":[{"purpose":"Locate input","tool":"filesystem.find_paths"}],
                "id":"discover","purpose":"Locate input","requiredEvidence":["Input path"]
            },
            {
                "completionEvidence":"Image loaded",
                "expectedCalls":[{"purpose":"Load input","tool":"filesystem.inspect_image"}],
                "id":"load","purpose":"Load input","requiredEvidence":["Input values"]
            }
        ]),
    );
    let mut policy = PlanPolicy::default();
    policy.accept(&plan, &all, 18)?;
    let mut discovery = operation("discover", "filesystem.find_paths");
    discovery.arguments.insert(
        "queries".to_owned(),
        serde_json::json!([{"nameContains":"input"}]),
    );
    policy.record_results(
        std::slice::from_ref(&discovery),
        &[Ok(completed_find_paths_operation(
            "discover",
            &[(1, false)],
        ))],
    );
    assert_eq!(
        policy.current_step_unfinished_tools(),
        vec!["filesystem.inspect_image"]
    );

    let mut refinement = operation("refine", "filesystem.find_paths");
    refinement.arguments.insert(
        "queries".to_owned(),
        serde_json::json!([{"nameContains":"input-table"}]),
    );
    assert!(policy.authorize_safe_read_recovery(std::slice::from_ref(&refinement), &all));
    assert_eq!(
        policy.current_step_unfinished_tools(),
        vec!["filesystem.find_paths"]
    );
    assert_eq!(policy.tool_surface_revision(), 0);

    let novel_read = operation("manifest", "filesystem.read_text");
    assert!(policy.authorize_safe_read_recovery(std::slice::from_ref(&novel_read), &all));
    assert_eq!(policy.tool_surface_revision(), 1);
    assert!(!policy.authorize_safe_read_recovery(std::slice::from_ref(&novel_read), &all));
    assert_eq!(
        policy.tool_surface_revision(),
        1,
        "an already-visible safe read must not invalidate the prefix again"
    );
    assert!(
        policy
            .safe_read_escape_tools()
            .contains(&"filesystem.read_text")
    );
    assert!(
        policy
            .pre_execution_evidence_deviation(&[novel_read])
            .is_none()
    );
    let completed_novel_read = operation("manifest-complete", "filesystem.read_text");
    policy.record_results(
        std::slice::from_ref(&completed_novel_read),
        &[Ok(completed_operation(
            "manifest-complete",
            ToolName::FilesystemReadText,
        ))],
    );
    assert!(
        policy
            .safe_read_escape_tools()
            .contains(&"filesystem.read_text"),
        "the capability stays visible so a stale cursor cannot recur"
    );
    assert!(
        policy
            .pre_execution_evidence_deviation(&[completed_novel_read])
            .is_some(),
        "the one-result scope bypass is consumed and repetition must return or revise"
    );
    Ok(())
}

#[test]
fn forecast_scope_is_schema_bound_and_rejects_serialized_composites() -> Result<(), IpcError> {
    let all = [
        tool(PLAN_TOOL),
        tool_with_parameters(
            "computation.run",
            &serde_json::json!({
                "inputs":{
                    "type":"array",
                    "items":{
                        "type":"object",
                        "properties":{"name":{"type":"string"}}
                    }
                },
                "limit":{"type":"integer"}
            }),
        ),
    ];
    let scoped_plan = |argument: &str, value: &str| {
        let mut plan = plan_call("initial", &["computation.run"]);
        plan.arguments.insert(
            "steps".to_owned(),
            serde_json::json!([{
                "completionEvidence":"Computation completed",
                "expectedCalls":[{
                    "purpose":"Run a bounded calculation",
                    "scope":{"argument":argument,"value":value},
                    "tool":"computation.run"
                }],
                "id":"compute","purpose":"Calculate result","requiredEvidence":["Typed input"]
            }]),
        );
        plan
    };

    PlanPolicy::default().accept(&scoped_plan("inputs", "principal"), &all, 8)?;

    let Err(error) =
        PlanPolicy::default().accept(&scoped_plan("inputs", r#"[{"name":"principal"}]"#), &all, 8)
    else {
        return Err(IpcError::invalid_request(
            "serialized scope composite was accepted",
        ));
    };
    assert!(error.message.contains("serialized JSON object or array"));
    assert!(error.message.contains("atomic string discriminator"));

    let Err(error) = PlanPolicy::default().accept(&scoped_plan("limit", "10"), &all, 8) else {
        return Err(IpcError::invalid_request(
            "non-string scope argument was accepted",
        ));
    };
    assert!(error.message.contains("contains no string discriminator"));

    let Err(error) = PlanPolicy::default().accept(&scoped_plan("missing", "principal"), &all, 8)
    else {
        return Err(IpcError::invalid_request(
            "unknown scope argument was accepted",
        ));
    };
    assert!(error.message.contains("unknown argument `missing`"));
    Ok(())
}

#[test]
fn parallel_scoped_calls_advance_only_in_forecast_order() -> Result<(), IpcError> {
    let all = [
        tool(PLAN_TOOL),
        tool_with_parameters(
            "filesystem.read_text",
            &serde_json::json!({"path":{"type":"string"}}),
        ),
    ];
    let mut plan = plan_with_step_count(2);
    plan.arguments.insert(
        "steps".to_owned(),
        serde_json::json!([
            {"completionEvidence":"A","expectedCalls":[{"purpose":"A","scope":{"argument":"path","value":"a.txt"},"tool":"filesystem.read_text"}],"id":"a","purpose":"A","requiredEvidence":["A"]},
            {"completionEvidence":"B","expectedCalls":[{"purpose":"B","scope":{"argument":"path","value":"b.txt"},"tool":"filesystem.read_text"}],"id":"b","purpose":"B","requiredEvidence":["B"]}
        ]),
    );
    let mut policy = PlanPolicy::default();
    policy.accept(&plan, &all, 18)?;
    let calls = [
        scoped_operation("a", "filesystem.read_text", "path", "a.txt"),
        scoped_operation("b", "filesystem.read_text", "path", "b.txt"),
    ];
    policy.record_results(
        &calls,
        &[
            Ok(completed_operation("a", ToolName::FilesystemReadText)),
            Ok(completed_operation("b", ToolName::FilesystemReadText)),
        ],
    );
    assert!(policy.forecast_complete());
    assert!(!policy.deviation_pending());
    Ok(())
}

#[test]
fn sixteen_steps_are_accepted_but_seventeen_gets_actionable_feedback() -> Result<(), IpcError> {
    let all = [tool(PLAN_TOOL), tool("filesystem.read_text")];
    let mut accepted_policy = PlanPolicy::default();
    accepted_policy.accept(&plan_with_step_count(MAX_PROVISIONAL_PLAN_STEPS), &all, 18)?;
    assert!(accepted_policy.active());

    let mut rejected_policy = PlanPolicy::default();
    let Err(error) = rejected_policy.accept(
        &plan_with_step_count(MAX_PROVISIONAL_PLAN_STEPS + 1),
        &all,
        18,
    ) else {
        return Err(IpcError::internal());
    };
    assert_eq!(error.code, IpcErrorCode::InvalidRequest);
    assert!(error.retryable);
    assert!(error.message.contains("contains 17 steps"));
    assert!(error.message.contains("maximum is 16"));
    assert!(error.message.contains("Merge related stages"));
    assert!(!rejected_policy.active());

    // A rejected governing plan leaves the gate closed, so an operation
    // emitted alongside it cannot become admissible on its own.
    assert!(
        rejected_policy
            .validate_calls(&[operation("read", "filesystem.read_text")])
            .is_err()
    );
    Ok(())
}

#[test]
fn file_outcomes_must_forecast_the_outcome_bearing_mutation() {
    let all = [tool(PLAN_TOOL), tool("filesystem.inspect_image")];
    let mut call = plan_call("initial", &["filesystem.inspect_image"]);
    call.arguments
        .insert("outcome".to_owned(), serde_json::json!("create_file"));
    let error = PlanPolicy::default()
        .accept(&call, &all, 12)
        .err()
        .map(|error| error.message)
        .unwrap_or_default();
    assert!(error.contains("declared create_file outcome"));
    assert!(error.contains("filesystem.write_text"));
    assert!(error.contains("filesystem.write_delimited_table"));

    let all = [tool(PLAN_TOOL), tool("filesystem.read_text")];
    let mut call = plan_call("initial", &["filesystem.read_text"]);
    call.arguments
        .insert("outcome".to_owned(), serde_json::json!("modify_file"));
    let error = PlanPolicy::default()
        .accept(&call, &all, 12)
        .err()
        .map(|error| error.message)
        .unwrap_or_default();
    assert!(error.contains("declared modify_file outcome"));
    assert!(error.contains("filesystem.replace_text"));
}

#[test]
fn one_plan_correction_reports_independent_scope_route_and_outcome_issues() -> Result<(), IpcError>
{
    let all = [
        tool(PLAN_TOOL),
        tool_with_parameters(
            "filesystem.find_paths",
            &serde_json::json!({
                "queries":{
                    "type":"array",
                    "items":{
                        "type":"object",
                        "properties":{"nameContains":{"type":"string"}}
                    }
                }
            }),
        ),
        tool("filesystem.inspect_image"),
        tool("filesystem.write_text"),
    ];
    let mut plan = plan_call("initial", &["filesystem.inspect_image"]);
    plan.arguments
        .insert("outcome".to_owned(), serde_json::json!("create_file"));
    plan.arguments.insert(
        "steps".to_owned(),
        serde_json::json!([
            {
                "completionEvidence":"Source located",
                "expectedCalls":[{
                    "purpose":"Locate source",
                    "scope":{"argument":"queries","value":"[{\"nameContains\":\"source\"}]"},
                    "tool":"filesystem.find_paths"
                }],
                "id":"source","purpose":"Locate source","requiredEvidence":["Source path"]
            },
            {
                "completionEvidence":"Template located",
                "expectedCalls":[{"purpose":"Locate template","tool":"filesystem.find_paths"}],
                "id":"template","purpose":"Locate template","requiredEvidence":["Template path"]
            },
            {
                "completionEvidence":"Image inspected",
                "expectedCalls":[{"purpose":"Inspect source","tool":"filesystem.inspect_image"}],
                "id":"inspect","purpose":"Inspect source","requiredEvidence":["Source values"]
            }
        ]),
    );

    let Err(error) = PlanPolicy::default().accept(&plan, &all, 12) else {
        return Err(IpcError::invalid_request(
            "independently malformed plan was accepted",
        ));
    };
    assert!(error.message.contains("2 independently detectable"));
    assert!(error.message.contains("serialized JSON object or array"));
    assert!(error.message.contains("declared create_file outcome"));
    Ok(())
}
