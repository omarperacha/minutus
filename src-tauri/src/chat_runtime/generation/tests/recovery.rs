use super::super::recovery_flow::mixed_read_only_argument_repair_allowed;
use super::*;

#[test]
fn safe_read_scope_recovery_preempts_terminal_phase_rejection() -> Result<(), IpcError> {
    let all_tools = [
        "orchestration.submit_plan",
        "filesystem.stat",
        "filesystem.find_paths",
        "filesystem.inspect_image",
    ]
    .into_iter()
    .map(|name| PromptToolDefinition {
        name: name.to_owned(),
        description: String::new(),
        parameters: serde_json::json!({}),
    })
    .collect::<Vec<_>>();
    let plan = ParsedToolCall {
        call_id: "plan".to_owned(),
        recipient: "orchestration.submit_plan".to_owned(),
        name: "orchestration.submit_plan".to_owned(),
        arguments: BTreeMap::from([
            ("assumptions".to_owned(), serde_json::json!([])),
            ("mode".to_owned(), serde_json::json!("initial")),
            ("outcome".to_owned(), serde_json::json!("answer")),
            (
                "steps".to_owned(),
                serde_json::json!([{
                    "completionEvidence":"Metadata loaded",
                    "expectedCalls":[{"purpose":"Read metadata","tool":"filesystem.stat"}],
                    "id":"metadata","purpose":"Read metadata","requiredEvidence":["Metadata"]
                }]),
            ),
            (
                "successCriteria".to_owned(),
                serde_json::json!(["The requested answer is supported"]),
            ),
            ("target".to_owned(), serde_json::json!("Answer the request")),
        ]),
    };
    let mut state = GenerationToolState::default();
    state.plan_policy.accept(&plan, &all_tools, 8)?;
    let current_tools = state.plan_policy.tools(&all_tools);
    assert!(current_tools.iter().all(|tool| !matches!(
        tool.name.as_str(),
        "filesystem.find_paths" | "filesystem.inspect_image"
    )));
    let calls = vec![
        ParsedToolCall {
            call_id: "discover".to_owned(),
            recipient: "filesystem.find_paths".to_owned(),
            name: "filesystem.find_paths".to_owned(),
            arguments: BTreeMap::from([(
                "queries".to_owned(),
                serde_json::json!("malformed-but-bounded"),
            )]),
        },
        ParsedToolCall {
            call_id: "inspect".to_owned(),
            recipient: "filesystem.inspect_image".to_owned(),
            name: "filesystem.inspect_image".to_owned(),
            arguments: BTreeMap::from([("path".to_owned(), serde_json::json!("source/table.png"))]),
        },
    ];
    assert!(!calls_available_in_phase(&current_tools, &calls));
    assert!(
        state
            .plan_policy
            .authorize_safe_read_recovery(&calls, &all_tools)
    );
    let recovered_tools = state.plan_policy.tools(&all_tools);
    assert!(calls_available_in_phase(&recovered_tools, &calls));
    assert_eq!(
        phase_call_availability(&recovered_tools, &state, &calls),
        (true, false)
    );
    Ok(())
}

#[test]
fn parallel_preplan_metadata_availability_does_not_validate_arguments() {
    let all_tools = [
        "web.retrieve",
        "web.search",
        "x402.request",
        "orchestration.submit_plan",
        "filesystem.find_paths",
    ]
    .into_iter()
    .map(|name| PromptToolDefinition {
        name: name.to_owned(),
        description: String::new(),
        parameters: serde_json::json!({}),
    })
    .collect::<Vec<_>>();
    let calls = [
        ("find-upload", "*", "/data/user/00__upload_target"),
        ("find-cashflow", "*cashflow*", "/data/user"),
        ("find-user", "*", "/data/user"),
    ]
    .into_iter()
    .map(|(call_id, pattern, root)| ParsedToolCall {
        call_id: call_id.to_owned(),
        recipient: "filesystem.find_paths".to_owned(),
        name: "filesystem.find_paths".to_owned(),
        arguments: BTreeMap::from([
            ("caseInsensitive".to_owned(), serde_json::json!("true")),
            ("maxDepth".to_owned(), serde_json::json!("4")),
            ("pattern".to_owned(), serde_json::json!(pattern)),
            ("root".to_owned(), serde_json::json!(root)),
        ]),
    })
    .collect::<Vec<_>>();
    let mut state = GenerationToolState::default();
    let initial_tools = state.plan_policy.tools(&all_tools);

    assert!(calls_available_in_phase(&initial_tools, &calls));
    assert!(
        calls
            .iter()
            .all(|call| { crate::chat_tools::validated_chat_tool_request(call, &[]).is_err() })
    );
    assert!(
        !state
            .plan_policy
            .authorize_safe_read_recovery(&calls, &all_tools)
    );
    assert_eq!(state.plan_policy.tools(&all_tools), initial_tools);
    assert_eq!(
        phase_call_availability(&initial_tools, &state, &calls),
        (true, false)
    );
}

#[test]
fn prior_plan_argument_repair_does_not_block_one_invalid_read_in_a_read_only_batch() {
    let calls = [
        ParsedToolCall {
            call_id: "list".to_owned(),
            recipient: "filesystem.list".to_owned(),
            name: "filesystem.list".to_owned(),
            arguments: BTreeMap::from([
                ("path".to_owned(), serde_json::json!("monthly cashflow")),
                ("workspace".to_owned(), serde_json::json!("primary")),
            ]),
        },
        ParsedToolCall {
            call_id: "read".to_owned(),
            recipient: "filesystem.read_text".to_owned(),
            name: "filesystem.read_text".to_owned(),
            arguments: BTreeMap::from([("path".to_owned(), serde_json::json!("ref:path-5"))]),
        },
    ];
    let mut state = GenerationToolState {
        argument_corrections: 1,
        ..Default::default()
    };
    let error = IpcError::invalid_request("workspace is required");
    assert!(mixed_read_only_argument_repair_allowed(&state, &calls, 1));
    assert!(
        state
            .request_recovery(
                RecoveryKind::InvalidArguments,
                "accepted-plan",
                argument_correction_fingerprint(&calls, &error),
                "Repair the read's workspace argument",
            )
            .is_ok()
    );
    record_argument_correction(&mut state, &calls, &error);
    assert_eq!(state.argument_corrections, 2);
    state.argument_corrections = MAX_ARGUMENT_CORRECTIONS;
    assert!(!mixed_read_only_argument_repair_allowed(&state, &calls, 1));
}

#[test]
fn rejected_plan_control_cannot_discard_a_valid_current_read() -> Result<(), IpcError> {
    let all_tools = ["orchestration.submit_plan", "web.search"]
        .into_iter()
        .map(|name| PromptToolDefinition {
            name: name.to_owned(),
            description: String::new(),
            parameters: serde_json::json!({}),
        })
        .collect::<Vec<_>>();
    let plan = ParsedToolCall {
        call_id: "plan".to_owned(),
        recipient: "orchestration.submit_plan".to_owned(),
        name: "orchestration.submit_plan".to_owned(),
        arguments: BTreeMap::from([
            ("assumptions".to_owned(), serde_json::json!([])),
            ("mode".to_owned(), serde_json::json!("initial")),
            ("outcome".to_owned(), serde_json::json!("answer")),
            (
                "steps".to_owned(),
                serde_json::json!([{
                    "completionEvidence":"The current fact is loaded",
                    "expectedCalls":[{"purpose":"Load the current fact","tool":"web.search"}],
                    "id":"evidence",
                    "purpose":"Load the current fact",
                    "requiredEvidence":["A current primary-source result"]
                }]),
            ),
            (
                "successCriteria".to_owned(),
                serde_json::json!(["The answer is supported by current evidence"]),
            ),
            ("target".to_owned(), serde_json::json!("Answer the request")),
        ]),
    };
    let mut policy = PlanPolicy::default();
    policy.accept(&plan, &all_tools, 8)?;
    let valid_read = ParsedToolCall {
        call_id: "current-read".to_owned(),
        recipient: "web.search".to_owned(),
        name: "web.search".to_owned(),
        arguments: BTreeMap::from([(
            "query".to_owned(),
            serde_json::json!("current stable compiler release"),
        )]),
    };

    assert!(current_reads_can_survive_rejected_plan(
        &[],
        &mut policy,
        std::slice::from_ref(&valid_read),
    ));
    assert!(!current_reads_can_survive_rejected_plan(
        &[],
        &mut policy,
        &[],
    ));
    Ok(())
}

#[test]
fn accumulated_corrections_do_not_force_deep_evidence_inference() -> Result<(), IpcError> {
    let all_tools = ["orchestration.submit_plan", "filesystem.stat"]
        .into_iter()
        .map(|name| PromptToolDefinition {
            name: name.to_owned(),
            description: String::new(),
            parameters: serde_json::json!({}),
        })
        .collect::<Vec<_>>();
    let plan = ParsedToolCall {
        call_id: "plan".to_owned(),
        recipient: "orchestration.submit_plan".to_owned(),
        name: "orchestration.submit_plan".to_owned(),
        arguments: BTreeMap::from([
            ("assumptions".to_owned(), serde_json::json!([])),
            ("mode".to_owned(), serde_json::json!("initial")),
            ("outcome".to_owned(), serde_json::json!("answer")),
            (
                "steps".to_owned(),
                serde_json::json!([{
                    "completionEvidence":"Metadata loaded",
                    "expectedCalls":[{"purpose":"Read metadata","tool":"filesystem.stat"}],
                    "id":"metadata","purpose":"Read metadata","requiredEvidence":["Metadata"]
                }]),
            ),
            (
                "successCriteria".to_owned(),
                serde_json::json!(["The answer uses current metadata"]),
            ),
            ("target".to_owned(), serde_json::json!("Answer the request")),
        ]),
    };
    let mut state = GenerationToolState {
        argument_corrections: 3,
        ..GenerationToolState::default()
    };
    state.plan_policy.accept(&plan, &all_tools, 8)?;
    let conversation = Conversation {
        id: ipc_types::ConversationId("conversation".to_owned()),
        project_id: ipc_types::ProjectId("project".to_owned()),
        title: "title".to_owned(),
        messages: Vec::new(),
        created_at: "2026-08-26T00:00:00Z".to_owned(),
        updated_at: "2026-08-26T00:00:00Z".to_owned(),
    };

    let profile = state.execution_profile(&conversation, 0, false);
    assert_eq!(profile.work, ExecutionWorkKind::Evidence);
    assert_eq!(profile.profile, agent_core::ExecutionProfile::Balanced);
    Ok(())
}

#[test]
fn argument_repair_keeps_structural_safety_while_checkpoint_policy_bounds_retries() {
    let invalid_collector = ParsedToolCall {
        call_id: "invalid-collector".to_owned(),
        recipient: "filesystem.collect_evidence".to_owned(),
        name: "filesystem.collect_evidence".to_owned(),
        arguments: BTreeMap::from([
            (
                "items".to_owned(),
                serde_json::json!([{
                    "id": "source",
                    "nameContains": "",
                    "requirement": "Locate the source image",
                    "operation": "find_matches"
                }]),
            ),
            ("workspace".to_owned(), serde_json::json!("primary")),
        ]),
    };
    let mut state = GenerationToolState::default();
    let first_error = IpcError::invalid_request("nameContains must not be empty");
    assert!(tool_argument_repair_allowed(
        &state,
        std::slice::from_ref(&invalid_collector),
        &first_error,
    ));
    let second_error = IpcError::invalid_request("operation is unknown");
    assert!(tool_argument_repair_allowed(
        &state,
        std::slice::from_ref(&invalid_collector),
        &second_error,
    ));
    record_argument_correction(
        &mut state,
        std::slice::from_ref(&invalid_collector),
        &first_error,
    );
    assert_eq!(state.argument_corrections, 1);
    state.argument_corrections = MAX_ARGUMENT_CORRECTIONS;
    assert!(!tool_argument_repair_allowed(
        &state,
        &[invalid_collector],
        &second_error,
    ));
    assert!(
        tool_preparation_terminal_error()
            .message
            .starts_with("Minutus could not safely repair")
    );
    assert!(
        !tool_preparation_terminal_error()
            .message
            .contains("invalid tool arguments")
    );
}
