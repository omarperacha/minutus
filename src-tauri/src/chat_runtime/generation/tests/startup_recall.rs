use super::super::recall_scope::RecallScope;
use super::*;

fn conversation_with_inputs(count: usize) -> Conversation {
    let mut conversation = empty_conversation("startup");
    for index in 0..count {
        conversation.messages.push(ipc_types::Message {
            id: ipc_types::MessageId(format!("input-{index}")),
            conversation_id: conversation.id.clone(),
            role: MessageRole::User,
            parts: vec![MessagePart::Text {
                text: format!("Input {index}"),
            }],
            created_at: conversation.created_at.clone(),
        });
    }
    conversation
}

fn tools() -> Vec<PromptToolDefinition> {
    [
        "orchestration.submit_plan",
        "orchestration.recall",
        "web.search",
        "filesystem.inspect_image",
        "filesystem.write_text",
    ]
    .into_iter()
    .map(prompt_tool)
    .collect()
}

#[test]
fn fresh_input_cannot_trigger_recall_but_planning_enables_later_recovery() -> Result<(), IpcError> {
    let conversation = conversation_with_inputs(1);
    let mut state = GenerationToolState {
        recall_scope: RecallScope::for_conversation(&conversation),
        ..GenerationToolState::default()
    };
    let all = tools();
    let projected = state.projected_tools(&all);
    assert!(
        !projected
            .iter()
            .any(|tool| tool.name == "orchestration.recall")
    );
    assert!(projected.iter().any(|tool| tool.name == "web.search"));
    let call = parsed_call("recall", "orchestration.recall");
    assert_eq!(
        phase_call_availability(&projected, &state, &[call]),
        (false, true)
    );
    // Prospective admission and searchable tool surfaces apply the same guard.
    let mut attempted_surface = all.clone();
    state.filter_unavailable_tools(&mut attempted_surface);
    assert!(
        !attempted_surface
            .iter()
            .any(|tool| tool.name == "orchestration.recall")
    );
    state.plan_policy.accept(&v6_file_plan(), &all, 8)?;
    assert!(
        state
            .projected_tools(&all)
            .iter()
            .any(|tool| tool.name == "orchestration.recall")
    );
    Ok(())
}

#[test]
fn prior_inputs_and_new_direct_web_results_keep_preplan_recall() {
    let conversation = conversation_with_inputs(2);
    let restored = GenerationToolState {
        recall_scope: RecallScope::for_conversation(&conversation),
        ..GenerationToolState::default()
    };
    assert!(restored.recall_available());
    let mut fresh = GenerationToolState {
        recall_scope: RecallScope::for_conversation(&conversation_with_inputs(1)),
        ..GenerationToolState::default()
    };
    fresh.preplan_result_call_ids.push("web-result".into());
    assert!(fresh.recall_available());
    assert!(
        fresh
            .projected_tools(&tools())
            .iter()
            .any(|tool| tool.name == "orchestration.recall")
    );
}

#[test]
fn a_current_attachment_does_not_create_fictitious_prior_history() {
    let mut conversation = conversation_with_inputs(1);
    conversation.messages[0].parts.push(MessagePart::Asset {
        asset_id: ipc_types::AssetId("image".into()),
    });
    assert!(matches!(
        RecallScope::for_conversation(&conversation),
        RecallScope::CurrentInputOnly
    ));
    conversation.messages[0].role = MessageRole::Assistant;
    assert!(matches!(
        RecallScope::for_conversation(&conversation),
        RecallScope::Historical
    ));
}

#[test]
fn first_preplan_result_establishes_one_recall_prefix_boundary() {
    let mut state = GenerationToolState {
        recall_scope: RecallScope::CurrentInputOnly,
        ..GenerationToolState::default()
    };
    let all = tools();
    let initial = state.projected_tools(&all);
    let before = state.provider_prefix_boundary();
    let mut tracker = ProviderPromptPrefixTracker::default();
    assert_eq!(tracker.observe(before, "policy initial tools"), None);
    assert!(state.admit_stable_execution_prefix(&initial).is_ok());

    // Both deferred PlanPending calls and direct-web results retain paired
    // pre-plan results. Those references make recall newly available.
    state.preplan_result_call_ids.push("preplan-result".into());
    let expanded = state.projected_tools(&all);
    assert!(
        expanded
            .iter()
            .any(|tool| tool.name == "orchestration.recall")
    );
    assert!(state.admit_stable_execution_prefix(&expanded).is_ok());
    let after = state.provider_prefix_boundary();
    assert_ne!(before, after);
    assert_eq!(tracker.observe(after, "policy expanded tools"), None);

    state.preplan_result_call_ids.push("second-result".into());
    assert_eq!(after, state.provider_prefix_boundary());
    assert!(state.admit_stable_execution_prefix(&expanded).is_ok());
    assert_eq!(
        tracker.observe(after, "policy expanded tools next"),
        Some("policy expanded tools".to_owned())
    );
    let mut altered = expanded;
    altered[0].description.push_str(" unexpected contract edit");
    assert!(state.admit_stable_execution_prefix(&altered).is_err());
}
