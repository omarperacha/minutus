use super::*;

#[test]
fn execution_prefix_identity_is_order_and_contract_sensitive() {
    let tools = vec![
        PromptToolDefinition {
            name: "filesystem.inspect_image".to_owned(),
            description: "Inspect one image".to_owned(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"],
                "additionalProperties": false
            }),
        },
        PromptToolDefinition {
            name: "orchestration.complete_stage".to_owned(),
            description: "Complete the current stage".to_owned(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": [],
                "additionalProperties": false
            }),
        },
    ];
    assert_eq!(
        execution_prefix_sha256(&tools),
        execution_prefix_sha256(&tools)
    );

    let mut reordered = tools.clone();
    reordered.reverse();
    assert_ne!(
        execution_prefix_sha256(&tools),
        execution_prefix_sha256(&reordered)
    );

    let mut rewritten = tools.clone();
    rewritten[0].description.push_str(" exactly");
    assert_ne!(
        execution_prefix_sha256(&tools),
        execution_prefix_sha256(&rewritten)
    );
}

#[test]
fn rendered_prefix_survives_stage_progression_and_resets_at_contract_boundaries() {
    let stable = ProviderPromptPrefixBoundary {
        plan_revision: 1,
        tool_surface_revision: 0,
        x402_attempt_consumed: false,
        recall_available: true,
    };
    let mut tracker = ProviderPromptPrefixTracker::default();

    assert_eq!(
        tracker.observe(stable, "policy tools history stage-one"),
        None
    );
    assert_eq!(
        tracker.observe(stable, "policy tools history stage-two"),
        Some("policy tools history stage-".to_owned())
    );

    let revised = ProviderPromptPrefixBoundary {
        plan_revision: 2,
        tool_surface_revision: 0,
        x402_attempt_consumed: false,
        recall_available: true,
    };
    assert_eq!(
        tracker.observe(revised, "policy tools revised stage-one"),
        None
    );

    let schema_loaded = ProviderPromptPrefixBoundary {
        plan_revision: 2,
        tool_surface_revision: 1,
        x402_attempt_consumed: false,
        recall_available: true,
    };
    assert_eq!(
        tracker.observe(schema_loaded, "policy expanded-tools stage-one"),
        None
    );
    assert_eq!(
        tracker.observe(schema_loaded, "policy expanded-tools stage-two"),
        Some("policy expanded-tools stage-".to_owned())
    );
}

#[test]
fn authorized_safe_read_expansion_establishes_one_new_stable_prefix() -> Result<(), IpcError> {
    let all_tools = [
        "filesystem.inspect_image",
        "filesystem.read_text",
        "filesystem.write_text",
        "orchestration.submit_plan",
        COMPLETE_STAGE_TOOL,
        SEARCH_TOOLS_TOOL,
    ]
    .map(prompt_tool);
    let mut state = GenerationToolState::default();
    state.plan_policy.accept(&v6_file_plan(), &all_tools, 12)?;

    let initial_tools = state.plan_policy.tools(&all_tools);
    assert!(
        !initial_tools
            .iter()
            .any(|tool| tool.name == "filesystem.read_text")
    );
    assert!(state.admit_stable_execution_prefix(&initial_tools).is_ok());

    let read = ParsedToolCall {
        call_id: "read-convention".to_owned(),
        recipient: "filesystem.read_text".to_owned(),
        name: "filesystem.read_text".to_owned(),
        arguments: BTreeMap::new(),
    };
    assert!(
        state
            .plan_policy
            .authorize_safe_read_recovery(std::slice::from_ref(&read), &all_tools)
    );
    assert_eq!(state.plan_policy.tool_surface_revision(), 1);

    let expanded_tools = state.plan_policy.tools(&all_tools);
    assert!(
        expanded_tools
            .iter()
            .any(|tool| tool.name == "filesystem.read_text")
    );
    assert!(state.admit_stable_execution_prefix(&expanded_tools).is_ok());
    assert!(state.admit_stable_execution_prefix(&expanded_tools).is_ok());
    Ok(())
}

#[test]
fn positioned_completion_then_verified_write_keeps_finish_or_continue_available()
-> Result<(), IpcError> {
    let all_tools = [
        "filesystem.inspect_image",
        "filesystem.write_text",
        "orchestration.submit_plan",
        COMPLETE_STAGE_TOOL,
        SEARCH_TOOLS_TOOL,
    ]
    .into_iter()
    .map(prompt_tool)
    .collect::<Vec<_>>();
    let mut state = GenerationToolState::default();
    state.plan_policy.accept(&v6_file_plan(), &all_tools, 12)?;

    let active_tools = state.plan_policy.tools(&all_tools);
    assert!(!active_tools.is_empty());
    assert!(state.admit_stable_execution_prefix(&active_tools).is_ok());

    let image_call = ParsedToolCall {
        call_id: "image".to_owned(),
        recipient: "filesystem.inspect_image".to_owned(),
        name: "filesystem.inspect_image".to_owned(),
        arguments: BTreeMap::new(),
    };
    state.plan_policy.record_results(
        std::slice::from_ref(&image_call),
        &[Ok(completed_tool_exchange(
            "image",
            ipc_types::ToolName::FilesystemInspectImage,
        ))],
    );
    state.plan_policy.complete_advisory_stage(&ParsedToolCall {
        call_id: "complete-extract".to_owned(),
        recipient: COMPLETE_STAGE_TOOL.to_owned(),
        name: COMPLETE_STAGE_TOOL.to_owned(),
        arguments: BTreeMap::new(),
    })?;

    // This is the live ORCH-001 ordering: complete_stage is the boundary,
    // and the dependency-ready verified write follows it in the same batch.
    let write_call = ParsedToolCall {
        call_id: "write".to_owned(),
        recipient: "filesystem.write_text".to_owned(),
        name: "filesystem.write_text".to_owned(),
        arguments: BTreeMap::new(),
    };
    state.plan_policy.record_results(
        std::slice::from_ref(&write_call),
        &[Ok(completed_tool_exchange(
            "write",
            ipc_types::ToolName::FilesystemWriteText,
        ))],
    );

    assert!(!state.plan_policy.final_response_ready());
    assert!(state.plan_policy.forecast_complete());
    assert!(!state.plan_policy.tools(&all_tools).is_empty());
    assert!(
        state
            .admit_stable_execution_prefix(&state.plan_policy.tools(&all_tools))
            .is_ok()
    );
    assert!(!state.plan_policy.allows_automatic_write_confirmation());
    Ok(())
}

#[test]
fn explicit_completed_answer_closes_tools_and_repairs_stale_control() -> Result<(), IpcError> {
    let all_tools = [
        "filesystem.inspect_image",
        "filesystem.write_text",
        "orchestration.submit_plan",
        COMPLETE_STAGE_TOOL,
        SEARCH_TOOLS_TOOL,
    ]
    .map(prompt_tool);
    let mut plan = v6_file_plan();
    plan.arguments
        .insert("outcome".to_owned(), serde_json::json!("answer"));
    plan.arguments
        .get_mut("steps")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or_else(IpcError::internal)?
        .truncate(1);
    let mut state = GenerationToolState::default();
    state.plan_policy.accept(&plan, &all_tools, 12)?;
    state.plan_policy.complete_advisory_stage(&ParsedToolCall {
        call_id: "done".to_owned(),
        recipient: COMPLETE_STAGE_TOOL.to_owned(),
        name: COMPLETE_STAGE_TOOL.to_owned(),
        arguments: BTreeMap::new(),
    })?;
    let final_tools = state.plan_policy.tools(&all_tools);
    assert!(final_tools.is_empty());
    assert!(state.admit_stable_execution_prefix(&final_tools).is_ok());
    for arguments in [
        BTreeMap::from([("stage".to_owned(), serde_json::json!("write_csv"))]),
        BTreeMap::from([
            ("arguments".to_owned(), serde_json::json!("{}")),
            ("stageId".to_owned(), serde_json::json!("write_csv")),
            ("workspace".to_owned(), serde_json::json!("primary")),
        ]),
    ] {
        let stale_completion = ParsedToolCall {
            call_id: "stale-final-completion".to_owned(),
            recipient: COMPLETE_STAGE_TOOL.to_owned(),
            name: COMPLETE_STAGE_TOOL.to_owned(),
            arguments,
        };
        assert!(final_response_tool_batch_repairable(
            &state,
            std::slice::from_ref(&stale_completion)
        ));
    }
    assert!(
        state
            .request_recovery(
                RecoveryKind::StalePhase,
                state.plan_policy.correction_scope(),
                "final-response-stale-control",
                "Respond to the user now in plain text.",
            )
            .is_ok()
    );
    let conversation = empty_conversation("final-response");
    let final_profile = state.execution_profile(&conversation, 0, false);
    assert_eq!(final_profile.work, ExecutionWorkKind::FinalResponse);
    assert_eq!(state.execution_state(true), ToolExecutionState::Completion);
    let mut final_round = ChatGenerationRound {
        reasoning: String::new(),
        content: "Created result/output.csv".to_owned(),
        calls: Vec::new(),
        reason: GenerationCompletion::Stop,
        image_ids: Vec::new(),
        binding_sha256: "a".repeat(64),
        timing: RoundTiming::default(),
    };
    assert!(matches!(
        terminal_without_tool_calls(&mut final_round, &state.plan_policy),
        Some(ChatTerminal::Final { text, .. }) if text == "Created result/output.csv"
    ));
    Ok(())
}

#[test]
fn unknown_tool_recovery_is_bounded_for_parallel_batches() {
    let tools = vec![PromptToolDefinition {
        name: "orchestration.submit_plan".to_owned(),
        description: String::new(),
        parameters: serde_json::json!({}),
    }];
    let unknown = ParsedToolCall {
        call_id: "unknown".to_owned(),
        recipient: "project.browse_entries".to_owned(),
        name: "project.browse_entries".to_owned(),
        arguments: BTreeMap::new(),
    };
    let state = GenerationToolState::default();
    assert_eq!(
        phase_call_availability(&tools, &state, std::slice::from_ref(&unknown)),
        (false, true)
    );

    let legacy_counter_exhausted = GenerationToolState {
        registry_corrections: 3,
        ..GenerationToolState::default()
    };
    assert_eq!(
        phase_call_availability(
            &tools,
            &legacy_counter_exhausted,
            std::slice::from_ref(&unknown)
        ),
        (false, true)
    );
    assert_eq!(
        phase_call_availability(&tools, &state, &[unknown.clone(), unknown]),
        (false, true)
    );
}

#[test]
fn golden_registry_then_malformed_inactive_call_gets_separate_phase_correction() {
    let tools = vec![PromptToolDefinition {
        name: "orchestration.submit_plan".to_owned(),
        description: String::new(),
        parameters: serde_json::json!({}),
    }];
    let inactive = ParsedToolCall {
        call_id: "inactive-malformed-find".to_owned(),
        recipient: "filesystem.find_paths".to_owned(),
        name: "filesystem.find_paths".to_owned(),
        arguments: BTreeMap::from([(
            "queries".to_owned(),
            serde_json::json!([{
                "nameContains": "",
                "relativePath": "outputs/reconstructed_tables/monthly cashflow"
            }]),
        )]),
    };
    let mut state = GenerationToolState {
        registry_corrections: 1,
        ..GenerationToolState::default()
    };
    assert!(inactive_tool_batch_repairable(std::slice::from_ref(
        &inactive
    )));
    assert_eq!(
        phase_call_availability(&tools, &state, std::slice::from_ref(&inactive)),
        (false, true)
    );
    state
        .phase_correction_scopes
        .insert(state.plan_policy.correction_scope(), 3);
    assert_eq!(
        phase_call_availability(&tools, &state, std::slice::from_ref(&inactive)),
        (false, true)
    );
    assert_eq!(
        phase_call_availability(
            &tools,
            &GenerationToolState::default(),
            &[inactive.clone(), inactive]
        ),
        (false, true)
    );
}
