use super::*;

#[test]
fn generic_upstream_failure_uses_the_sanitized_provider_class() {
    for (error, expected) in [
        (
            model_runtime::OllamaError::DeadlineExceeded,
            ModelOutputFailureKind::ProviderTimeout,
        ),
        (
            model_runtime::OllamaError::Unavailable,
            ModelOutputFailureKind::ProviderUnavailable,
        ),
        (
            model_runtime::OllamaError::InvalidResponse,
            ModelOutputFailureKind::InvalidProviderResponse,
        ),
    ] {
        assert_eq!(
            resolved_stream_failure(
                &Err(AppCoreError::Ollama(error)),
                Some(ModelOutputFailureKind::UpstreamFailure),
            ),
            Some(expected)
        );
    }
    assert_eq!(
        resolved_stream_failure(&Ok(()), Some(ModelOutputFailureKind::InvalidProtocol),),
        Some(ModelOutputFailureKind::InvalidProtocol)
    );
}

#[test]
fn reasoning_echo_filter_suppresses_only_an_exact_chunked_prompt_prefix() {
    let mut echoed = ReasoningEchoFilter::new(Some("Explain ζ in detail"));
    assert_eq!(echoed.push("Explain "), None);
    assert_eq!(echoed.push("ζ in detail\n\n"), None);
    assert_eq!(
        echoed.push("We need a concise explanation."),
        Some("We need a concise explanation.".to_owned())
    );

    let mut genuine = ReasoningEchoFilter::new(Some("Explain ζ in detail"));
    assert_eq!(
        genuine.push("Consider the audience first."),
        Some("Consider the audience first.".to_owned())
    );
    assert_eq!(
        genuine.push(" Then answer."),
        Some(" Then answer.".to_owned())
    );
}

#[test]
fn terminal_errors_after_a_mutation_report_the_applied_change() {
    let ordinary = terminal_error(
        tool_loop_limit_error(),
        GenerationFailureCategory::ToolLimit,
    );
    let ordinary = match terminal_after_mutation(ordinary, false) {
        ChatTerminal::Error { error, .. } => Some(error),
        _ => None,
    };
    assert_eq!(
        ordinary.as_ref().map(|error| error.message.as_str()),
        Some("The response reached its workspace operation limit before completion")
    );

    for terminal in [
        terminal_error(
            tool_loop_limit_error(),
            GenerationFailureCategory::ToolLimit,
        ),
        ChatTerminal::Cancelled,
    ] {
        let applied = match terminal_after_mutation(terminal, true) {
            ChatTerminal::Error { error, .. } => Some(error),
            _ => None,
        };
        assert!(applied.as_ref().is_some_and(|error| {
            error
                .message
                .starts_with("The workspace change was applied")
        }));
        assert!(applied.is_some_and(|error| error.retryable));
    }
}

#[test]
fn incomplete_typed_outcome_reaches_the_typed_recovery_coordinator() {
    assert!(should_schedule_outcome_recovery(true, true, 4));
    assert!(!should_schedule_outcome_recovery(false, true, 4));
    assert!(!should_schedule_outcome_recovery(true, false, 4));
    assert!(!should_schedule_outcome_recovery(
        true,
        true,
        MAX_CHAT_TOOL_ROUNDS,
    ));
}

#[test]
fn missing_legacy_forecast_uses_forecast_recovery_only() {
    assert!(should_schedule_forecast_recovery(true, true, 4));
    assert!(!should_schedule_forecast_recovery(false, true, 4));
    assert!(!should_schedule_forecast_recovery(true, false, 4));
    assert!(!should_schedule_forecast_recovery(
        true,
        true,
        MAX_CHAT_TOOL_ROUNDS,
    ));
}

#[test]
fn applied_change_confirmation_is_host_generated_from_validated_paths() {
    assert_eq!(
        applied_change_confirmation(&["cv.html".to_owned()]),
        "Done — updated cv.html."
    );
    assert_eq!(
        applied_change_confirmation(&["a.txt".to_owned(), "b.txt".to_owned()]),
        "Done — updated 2 workspace paths: a.txt, b.txt."
    );
}

#[test]
fn bounded_stage_registry_keeps_only_required_operation_and_revision_escape() {
    let tools = [
        "orchestration.submit_plan",
        "filesystem.read_text",
        "filesystem.write_text",
        "computation.run",
    ]
    .into_iter()
    .map(|name| PromptToolDefinition {
        name: name.to_owned(),
        description: String::new(),
        parameters: serde_json::json!({}),
    })
    .collect::<Vec<_>>();
    let contract = BoundedStageContract::test_fixture(
        "filesystem.write_text",
        app_core::ToolStagePostcondition::FileWriteVerified,
    );
    let active = bounded_stage_tools(tools.clone(), Some(&contract));
    assert_eq!(
        active
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        ["orchestration.submit_plan", "filesystem.write_text"]
    );
    assert_eq!(bounded_stage_tools(tools.clone(), None), tools);
}

#[test]
fn bounded_stage_failures_are_counted_per_revision_and_operation() {
    let contract = BoundedStageContract::test_fixture(
        "filesystem.write_text",
        app_core::ToolStagePostcondition::FileWriteVerified,
    );
    let mut state = GenerationToolState {
        active_stage_contract: Some(contract.clone()),
        ..GenerationToolState::default()
    };
    state.suspend_bounded_stage();
    let key = state.bounded_stage_key(&contract);
    assert_eq!(state.bounded_stage_failures.get(&key), Some(&1));
    assert!(state.active_stage_contract.is_none());

    state.active_stage_contract = Some(contract.clone());
    state.suspend_bounded_stage();
    assert_eq!(state.bounded_stage_failures.get(&key), Some(&2));
    assert_eq!(BoundedStageContract::retry_limit(), 2);
}

#[test]
fn dependent_discovery_has_room_for_conversion_before_hard_limits() {
    // The observed eight-read trace still has bounded room for image
    // inspection, a CSV write and one recovery/validation round.
    assert!(tool_batch_within_limits(8, 8, 1));
    assert!(tool_batch_within_limits(9, 9, 1));
    assert!(tool_batch_within_limits(10, 10, 1));
    assert!(!tool_batch_within_limits(12, 12, 1));
    assert!(!tool_batch_within_limits(11, 18, 1));
}

#[test]
fn cashflow_reconstruction_eval_constrains_image_exhaustion_recovery() -> Result<(), IpcError> {
    // ORCH-001 batch 1 accepts the provisional plan and collects the PNG and
    // manifest. Batch 2 submits one closed representation; the host serializes
    // it and the ordinary write broker verifies the result.
    assert!(tool_batch_within_limits(0, 0, 2));
    assert!(tool_batch_within_limits(1, 2, 1));
    assert_eq!(MAX_CHAT_TOOL_ROUNDS.saturating_sub(2), 10);
    assert_eq!(MAX_CHAT_TOOL_CALLS.saturating_sub(3), 15);
    assert_eq!(remaining_plan_forecast_calls(2, 2), 17);

    let tools = [
        "orchestration.submit_plan",
        "orchestration.complete_stage",
        "filesystem.inspect_image",
        "filesystem.write_delimited_table",
    ]
    .into_iter()
    .map(prompt_tool)
    .collect::<Vec<_>>();
    let plan = ParsedToolCall {
        call_id: "cashflow-plan".to_owned(),
        recipient: "orchestration.submit_plan".to_owned(),
        name: "orchestration.submit_plan".to_owned(),
        arguments: BTreeMap::from([
            ("assumptions".to_owned(), serde_json::json!([])),
            (
                "constraints".to_owned(),
                serde_json::json!(["Preserve source values"]),
            ),
            (
                "contractVersion".to_owned(),
                serde_json::json!(crate::chat_tools::SEMANTIC_PLAN_CONTRACT_VERSION),
            ),
            ("mode".to_owned(), serde_json::json!("initial")),
            ("outcome".to_owned(), serde_json::json!("create_file")),
            (
                "steps".to_owned(),
                serde_json::json!([
                    {
                        "actionClass":"read", "dependsOn":[],
                        "doneWhen":"Every row and value is extracted", "id":"extract",
                        "objective":"Extract the image table",
                        "suggestedTools":["filesystem.inspect_image"]
                    },
                    {
                        "actionClass":"workspace_write", "dependsOn":["extract"],
                        "doneWhen":"The CSV exists", "id":"write",
                        "objective":"Write the complete CSV",
                        "suggestedTools":["filesystem.write_delimited_table"]
                    }
                ]),
            ),
            (
                "successCriteria".to_owned(),
                serde_json::json!(["A faithful CSV exists"]),
            ),
            (
                "target".to_owned(),
                serde_json::json!("Create the monthly cashflow CSV"),
            ),
        ]),
    };

    // Regression for requests 01a030ca-c445-7d70-8f57-ca5f9ae7ec87 and
    // 01a0491a-264a-70a1-bdc5-b1c6f35642d5: a rich image pass exhausted
    // every token in private reasoning and the recovery repeated it.
    let mut recovery = GenerationToolState::default();
    recovery.plan_policy.accept(&plan, &tools, 8)?;
    let diagnostic = output_exhaustion_recovery_diagnostic(&recovery.plan_policy);
    assert!(diagnostic.contains("orchestration.complete_stage"));
    assert!(
        recovery
            .request_recovery(
                RecoveryKind::OutputExhaustion,
                recovery.plan_policy.correction_scope(),
                "execution_mode:length:1",
                diagnostic,
            )
            .is_ok()
    );
    recovery.begin_outcome_recovery(GenerationCompletion::Length, &[]);
    assert!(recovery.outcome_recovery_used);
    assert!(recovery.outcome_recovery_from_length);
    let conversation = Conversation {
        id: ipc_types::ConversationId("cashflow-recovery".to_owned()),
        project_id: ipc_types::ProjectId("project".to_owned()),
        title: "cashflow".to_owned(),
        messages: Vec::new(),
        created_at: "2026-08-28T00:00:00Z".to_owned(),
        updated_at: "2026-08-28T00:00:00Z".to_owned(),
    };
    let decision = recovery.execution_profile(&conversation, 0, false);
    assert_eq!(decision.work, ExecutionWorkKind::Recovery);
    assert_eq!(decision.profile, agent_core::ExecutionProfile::Latency);
    assert!(decision.bounded_action);
    assert!(diagnostic.contains("Do not repeat transcription"));
    assert!(!diagnostic.contains("outcome-bearing operation"));
    assert!(should_schedule_outcome_recovery(true, true, 5,));
    Ok(())
}

#[test]
fn overdue_stage_decision_uses_a_bounded_low_reasoning_pass() {
    let decision = constrain_action_profile(
        ExecutionProfileDecision {
            profile: agent_core::ExecutionProfile::Deep,
            work: ExecutionWorkKind::Evidence,
            policy_version: agent_core::EXECUTION_PROFILE_POLICY_VERSION,
            bounded_action: false,
            bulk_arguments: false,
        },
        false,
        true,
    );
    assert_eq!(decision.profile, agent_core::ExecutionProfile::Latency);
    assert_eq!(decision.work, ExecutionWorkKind::Evidence);
    assert!(decision.bounded_action);
}
