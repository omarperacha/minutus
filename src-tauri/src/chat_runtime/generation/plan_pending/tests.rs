use super::*;

fn call(id: &str, value: &str) -> ParsedToolCall {
    ParsedToolCall {
        call_id: id.into(),
        recipient: "filesystem.write_text".into(),
        name: "filesystem.write_text".into(),
        arguments: BTreeMap::from([("text".into(), serde_json::json!(value))]),
    }
}

#[test]
fn workspace_path_discovery_does_not_admit_content_or_effects() {
    let state = GenerationToolState::default();
    let names = [
        "filesystem.list",
        "filesystem.find_paths",
        "filesystem.read_text",
        "filesystem.inspect_image",
        "filesystem.write_text",
        "computation.run",
        "orchestration.submit_plan",
    ];
    let all = names.map(|name| PromptToolDefinition {
        name: name.into(),
        description: String::new(),
        parameters: serde_json::json!({}),
    });
    let exposed = state.plan_policy.tools(&all);
    for name in names {
        let expected = matches!(name, "filesystem.list" | "filesystem.find_paths");
        let mut operation = call("path-metadata", "unused");
        operation.name = name.into();
        operation.recipient = name.into();
        if name != "orchestration.submit_plan" {
            assert_eq!(exposed.iter().any(|tool| tool.name == name), expected);
            assert_eq!(
                !requires_deferral(&state, std::slice::from_ref(&operation)),
                expected
            );
        }
        if expected {
            assert!(requires_deferral(
                &state,
                &[operation, call("write", "value")]
            ));
        }
    }
    assert!(!state.plan_policy.active());
}

#[test]
#[cfg(debug_assertions)]
fn metadata_discovery_precedes_planning_but_effects_and_mixed_batches_do_not()
-> Result<(), Box<dyn std::error::Error>> {
    let state = GenerationToolState::default();
    let mut discovery = ParsedToolCall {
        call_id: "metadata".into(),
        recipient: "x402.request".into(),
        name: "x402.request".into(),
        arguments: serde_json::from_value(serde_json::json!({
            "operation":"discover", "url":"http://127.0.0.1:5001/test-project/us-central1/api/x402/resources", "reason":"Read resource requirements"
        }))?,
    };
    assert!(!requires_deferral(&state, std::slice::from_ref(&discovery)));
    assert!(requires_deferral(
        &state,
        &[discovery.clone(), call("write", "value")]
    ));
    for operation in ["purchase", "purchase_url", "unknown"] {
        discovery
            .arguments
            .insert("operation".into(), serde_json::json!(operation));
        assert!(requires_deferral(&state, std::slice::from_ref(&discovery)));
    }
    assert!(!state.plan_policy.active());
    assert!(!state.operation_ledger.x402_attempt_consumed());
    Ok(())
}

fn error_message(result: Result<(), ChatTerminal>) -> Result<String, &'static str> {
    match result {
        Err(ChatTerminal::Error { error, .. }) => Ok(error.message),
        _ => Err("expected error"),
    }
}

#[test]
fn repeated_intent_reuses_original_linkage_and_its_own_retry_allowance()
-> Result<(), Box<dyn std::error::Error>> {
    let mut state = GenerationToolState::default();
    assert!(
        state
            .register_pending(&[call("first", "secret")], 0)
            .is_ok()
    );
    assert!(
        state
            .register_pending(&[call("second", "secret")], 1)
            .is_ok()
    );
    let pending = state.plan_pending.as_ref().ok_or("checkpoint")?;
    assert_eq!(pending.operations[0].original_call_id, "first");
    assert_eq!(pending.operations[0].argument_sha256.len(), 64);
    assert!(!format!("{pending:?}").contains("secret"));
    let diagnostic = pending.trace_projection().to_string();
    assert!(!diagnostic.contains("secret"));
    assert!(!diagnostic.contains("\"arguments\""));
    assert!(diagnostic.contains("argumentSha256"));
    let error = error_message(state.register_pending(&[call("third", "secret")], 2))?;
    assert!(error.contains("plan_not_submitted: unchanged_repetition"));
    assert_eq!(state.total_calls, 3);
    assert_eq!(state.argument_corrections, 0);
    assert_eq!(state.registry_corrections, 0);
    assert!(state.phase_correction_scopes.is_empty());
    assert!(state.recovery.pending().is_none());
    assert_eq!(state.operation_ledger.useful_result_sequence(), 0);
    Ok(())
}

#[test]
fn changed_inputs_and_order_update_intent_without_resetting_global_budgets()
-> Result<(), Box<dyn std::error::Error>> {
    let mut state = GenerationToolState::default();
    assert!(
        state
            .register_pending(&[call("a", "one"), call("b", "two")], 0)
            .is_ok()
    );
    assert!(
        state
            .register_pending(&[call("c", "two"), call("d", "one")], 1)
            .is_ok()
    );
    assert_eq!(
        state.plan_pending.as_ref().ok_or("pending")?.operations[0].original_call_id,
        "c"
    );
    assert_eq!(state.total_calls, 4);
    state.total_calls = MAX_CHAT_TOOL_CALLS - 1;
    assert!(
        error_message(state.register_pending(&[call("last", "changed")], 2))?
            .contains("call_limit")
    );
    assert!(
        error_message(
            state.register_pending(&[call("round", "changed again")], MAX_CHAT_TOOL_ROUNDS)
        )?
        .contains("round_limit")
    );
    Ok(())
}

#[test]
fn canonical_arguments_ignore_object_key_order() -> Result<(), Box<dyn std::error::Error>> {
    let mut a = call("a", "one");
    let mut b = call("b", "one");
    a.arguments
        .insert("nested".into(), serde_json::from_str(r#"{"b":2,"a":1}"#)?);
    b.arguments
        .insert("nested".into(), serde_json::from_str(r#"{"a":1,"b":2}"#)?);
    let mut state = GenerationToolState::default();
    assert!(state.register_pending(&[a], 0).is_ok());
    assert!(state.register_pending(&[b], 1).is_ok());
    assert_eq!(state.plan_pending.ok_or("pending")?.unchanged_retries, 1);
    Ok(())
}

#[test]
fn exits_preserve_pending_boundary_cause_and_cancellation() -> Result<(), Box<dyn std::error::Error>>
{
    let pending = PlanPending::from_calls(&[call("a", "one")]);
    assert!(matches!(
        annotate_terminal(ChatTerminal::Cancelled, Some(&pending)),
        ChatTerminal::Cancelled
    ));
    let terminal = annotate_terminal(
        ChatTerminal::Final {
            reasoning: None,
            text: "Done".into(),
            reason: FinishReason::Stop,
            mutation_applied: false,
        },
        Some(&pending),
    );
    let ChatTerminal::Error { error, .. } = terminal else {
        return Err("pending must not report success".into());
    };
    assert!(error.message.contains("model_stopped"));
    assert!(error.message.contains("Earlier completed work is retained"));
    let terminal = annotate_terminal(
        terminal_error(IpcError::internal(), GenerationFailureCategory::Persistence),
        Some(&pending),
    );
    assert!(
        matches!(terminal, ChatTerminal::Error { error, category: GenerationFailureCategory::Persistence } if error.message.contains("requiredNextOperation"))
    );
    Ok(())
}

#[test]
fn acceptance_clears_pending_retains_context_and_requires_fresh_calls() -> Result<(), IpcError> {
    let mut state = GenerationToolState::default();
    assert!(
        state
            .register_pending(&[call("deferred", "one")], 0)
            .is_ok()
    );
    let plan = ParsedToolCall { call_id: "plan".into(), recipient: PLAN_REQUIRED_OPERATION.into(), name: PLAN_REQUIRED_OPERATION.into(),
        arguments: serde_json::from_value(serde_json::json!({
            "contractVersion":"semantic_execution_v6", "mode":"initial", "outcome":"create_file", "target":"output.txt",
            "assumptions":[], "constraints":[], "successCriteria":["File created"],
            "steps":[{"id":"write", "actionClass":"workspace_write", "objective":"Create file", "doneWhen":"File verified", "dependsOn":[], "suggestedTools":["filesystem.write_text"]}]
        })).map_err(|_| IpcError::internal())? };
    let tools =
        [PLAN_REQUIRED_OPERATION, "filesystem.write_text"].map(|name| PromptToolDefinition {
            name: name.into(),
            description: String::new(),
            parameters: serde_json::json!({}),
        });
    dispatch::admit_plan_control(&mut state, &plan, &tools, 1)?;
    state.retain_preplan_results();
    assert!(state.plan_pending.is_none());
    assert!(!requires_deferral(&state, &[call("fresh", "one")]));
    assert!(state.plan_policy.retains_context_result("deferred"));
    assert_eq!(state.operation_ledger.useful_result_sequence(), 0);
    assert_eq!(state.total_calls, 1);
    assert!(
        state
            .plan_policy
            .validate_calls(&[call("fresh", "one")])
            .is_ok()
    );
    Ok(())
}

#[test]
fn task_admission_precedes_phase_and_schema_repair_but_preserves_web_bypass() {
    let state = GenerationToolState::default();
    for operation in [
        "filesystem.read_text",
        "filesystem.write_text",
        "computation.run",
        "x402.request",
    ] {
        let mut task = call("task", "unchanged original arguments");
        task.name = operation.into();
        task.recipient = operation.into();
        // Safe representation is enough for deferral. No workspace, task schema,
        // permission broker or privileged dispatcher is needed before a plan.
        assert!(requires_deferral(&state, &[task]));
    }
    let mut web = call("web", "query");
    web.name = "web.search".into();
    web.recipient.clone_from(&web.name);
    assert!(!requires_deferral(&state, std::slice::from_ref(&web)));
    assert!(requires_deferral(&state, &[web, call("write", "text")]));
    let mut control = call("plan", "bad plan");
    control.name = PLAN_REQUIRED_OPERATION.into();
    control.recipient.clone_from(&control.name);
    assert!(!requires_deferral(
        &state,
        &[control, call("write", "text")]
    ));
    let mut unknown = call("unknown", "text");
    unknown.name = "filesystem.not_a_tool".into();
    unknown.recipient.clone_from(&unknown.name);
    assert!(!requires_deferral(
        &state,
        &[unknown, call("write", "text")]
    ));
}

#[test]
fn pending_does_not_refresh_existing_general_recovery_capacity() {
    let mut state = GenerationToolState::default();
    for _ in 0..3 {
        assert!(
            state
                .request_recovery(RecoveryKind::InvalidArguments, "scope", "same", "repair")
                .is_ok()
        );
    }
    assert!(
        state
            .register_pending(&[call("pending", "intent")], 0)
            .is_ok()
    );
    assert!(
        state
            .request_recovery(RecoveryKind::InvalidArguments, "scope", "same", "repair")
            .is_err()
    );
    assert!(state.plan_pending.is_some());
}

#[test]
fn deferred_purchase_keeps_planning_admissible_without_executing_the_attempt() {
    let all = [
        "orchestration.submit_plan",
        "orchestration.recall",
        "x402.request",
    ]
    .map(|name| PromptToolDefinition {
        name: name.into(),
        description: String::new(),
        parameters: serde_json::json!({}),
    });
    let mut state = GenerationToolState {
        recall_scope: super::super::recall_scope::RecallScope::CurrentInputOnly,
        ..GenerationToolState::default()
    };
    let initial = state.projected_tools(&all);
    assert!(state.admit_stable_execution_prefix(&initial).is_ok());
    let deferred = ParsedToolCall {
        call_id: "purchase-intent".into(),
        recipient: "x402.request".into(),
        name: "x402.request".into(),
        arguments: BTreeMap::new(),
    };
    assert!(state.register_pending(&[deferred], 0).is_ok());
    let next = state.projected_tools(&all);
    assert!(
        next.iter()
            .any(|tool| tool.name == "orchestration.submit_plan")
    );
    assert!(next.iter().any(|tool| tool.name == "orchestration.recall"));
    assert!(state.admit_stable_execution_prefix(&next).is_ok());
    assert!(state.plan_pending.is_some());
    assert!(!state.plan_policy.active());
    assert!(!state.operation_ledger.x402_attempt_consumed());
    assert_eq!(state.operation_ledger.useful_result_sequence(), 0);
    assert_eq!(state.total_calls, 1);
}
