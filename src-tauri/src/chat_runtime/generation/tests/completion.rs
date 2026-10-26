//! Task finalization is a model decision, not an advisory-cursor cleanup loop.

use super::*;

fn tools() -> Vec<PromptToolDefinition> {
    [
        "orchestration.submit_plan",
        COMPLETE_STAGE_TOOL,
        SEARCH_TOOLS_TOOL,
        "filesystem.inspect_image",
        "filesystem.write_text",
    ]
    .map(prompt_tool)
    .to_vec()
}

fn write_result(state: &mut GenerationToolState, id: &str) {
    let call = ParsedToolCall {
        call_id: id.to_owned(),
        recipient: "filesystem.write_text".to_owned(),
        name: "filesystem.write_text".to_owned(),
        arguments: BTreeMap::new(),
    };
    state.record_results(
        &[call],
        &[Ok(completed_tool_exchange(
            id,
            ipc_types::ToolName::FilesystemWriteText,
        ))],
    );
}

#[test]
fn verified_write_allows_model_final_without_fabricating_read_completion() -> Result<(), IpcError> {
    let all = tools();
    let mut state = GenerationToolState::default();
    state.plan_policy.accept(&v6_file_plan(), &all, 12)?;
    write_result(&mut state, "write-before-checkpoint");
    assert_eq!(state.plan_policy.current_stage_id(), Some("extract"));
    assert!(!state.plan_policy.forecast_complete());
    assert!(!state.plan_policy.allows_automatic_write_confirmation());
    assert_eq!(state.plan_policy.tools(&all).len(), all.len());
    let profile = state.execution_profile(&empty_conversation("finished"), 0, false);
    assert_eq!(profile.profile, agent_core::ExecutionProfile::Latency);
    assert!(profile.bounded_action);
    assert_eq!(
        state
            .execution_profile(&empty_conversation("more-image-work"), 1, true)
            .profile,
        agent_core::ExecutionProfile::Balanced,
        "a previous write must not suppress reasoning for remaining image work"
    );
    let mut final_round = ChatGenerationRound {
        reasoning: String::new(),
        content: "Created the requested file.".to_owned(),
        calls: Vec::new(),
        reason: GenerationCompletion::Stop,
        image_ids: Vec::new(),
        binding_sha256: "a".repeat(64),
        timing: RoundTiming::default(),
    };
    assert!(matches!(
        terminal_without_tool_calls(&mut final_round, &state.plan_policy),
        Some(ChatTerminal::Final {
            mutation_applied: true,
            ..
        })
    ));
    assert_eq!(state.plan_policy.current_stage_id(), Some("extract"));
    Ok(())
}

#[test]
fn one_write_cannot_close_multiple_write_stages_or_the_task() -> Result<(), IpcError> {
    let all = tools();
    let mut plan = v6_file_plan();
    let steps = plan
        .arguments
        .get_mut("steps")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or_else(IpcError::internal)?;
    steps.push(serde_json::json!({
        "actionClass":"workspace_write", "dependsOn":["write"], "id":"second-write",
        "objective":"Create the second requested file", "doneWhen":"Both files are created",
        "suggestedTools":["filesystem.write_text"]
    }));
    let mut state = GenerationToolState::default();
    state.plan_policy.accept(&plan, &all, 12)?;
    let complete = ParsedToolCall {
        call_id: "complete".to_owned(),
        recipient: COMPLETE_STAGE_TOOL.to_owned(),
        name: COMPLETE_STAGE_TOOL.to_owned(),
        arguments: BTreeMap::new(),
    };
    state.plan_policy.complete_advisory_stage(&complete)?;
    write_result(&mut state, "first-write");
    assert_eq!(state.plan_policy.current_stage_id(), Some("write"));
    assert!(!state.plan_policy.allows_automatic_write_confirmation());
    state.plan_policy.complete_advisory_stage(&complete)?;
    assert_eq!(state.plan_policy.current_stage_id(), Some("second-write"));
    assert!(!state.plan_policy.final_response_ready());
    assert!(
        state
            .plan_policy
            .tools(&all)
            .iter()
            .any(|tool| tool.name == "filesystem.write_text")
    );
    Ok(())
}

#[test]
fn checkpoint_only_cannot_substitute_for_a_verified_write() -> Result<(), IpcError> {
    let mut state = GenerationToolState::default();
    state.plan_policy.accept(&v6_file_plan(), &tools(), 12)?;
    let complete = ParsedToolCall {
        call_id: "complete".to_owned(),
        recipient: COMPLETE_STAGE_TOOL.to_owned(),
        name: COMPLETE_STAGE_TOOL.to_owned(),
        arguments: BTreeMap::new(),
    };
    state.plan_policy.complete_advisory_stage(&complete)?;
    state.plan_policy.complete_advisory_stage(&complete)?;
    assert!(state.plan_policy.forecast_complete());
    assert!(!state.plan_policy.final_response_ready());
    assert!(state.plan_policy.requires_outcome_recovery());
    Ok(())
}
