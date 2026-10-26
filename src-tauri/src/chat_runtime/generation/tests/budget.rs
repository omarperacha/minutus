//! Budget-edge regressions span admission, accounting, positioned stages and finalization.

use super::*;

fn call(name: &str) -> ParsedToolCall {
    ParsedToolCall {
        call_id: name.to_owned(),
        recipient: name.to_owned(),
        name: name.to_owned(),
        arguments: BTreeMap::new(),
    }
}

fn active_state() -> Result<GenerationToolState, IpcError> {
    let mut state = GenerationToolState::default();
    let all = chat_tool_definitions(&workspaces())?;
    state
        .plan_policy
        .accept(&v6_file_plan(), &all, MAX_CHAT_TOOL_CALLS)?;
    Ok(state)
}

fn workspaces() -> Vec<crate::chat_tools::ChatToolWorkspace> {
    vec![crate::chat_tools::ChatToolWorkspace {
        alias: "primary".to_owned(),
        display_name: "fixture".to_owned(),
        grant_id: ipc_types::WorkspaceGrantId("grant".to_owned()),
        permission_level: ipc_types::WorkspacePermissionLevel::ReadAndWrite,
    }]
}

fn table_call() -> ParsedToolCall {
    let mut write = call("filesystem.write_delimited_table");
    write.arguments = BTreeMap::from([
        ("workspace".to_owned(), serde_json::json!("primary")),
        ("path".to_owned(), serde_json::json!("outputs/table.csv")),
        ("expectedSha256".to_owned(), serde_json::Value::Null),
        ("delimiter".to_owned(), serde_json::json!("comma")),
        ("columns".to_owned(), serde_json::json!(["label", "value"])),
        ("rows".to_owned(), serde_json::json!([["Example", 1]])),
    ]);
    write
}

#[test]
fn last_task_allowance_accepts_checkpoint_and_write_in_either_order() -> Result<(), IpcError> {
    for checkpoint_first in [true, false] {
        let mut state = active_state()?;
        // Earlier task/repair attempts still count. This reproduces the limiting
        // condition without relying on the particular discovery path taken.
        state.record_calls(&vec![call("filesystem.read_text"); 17]);
        let complete = call(COMPLETE_STAGE_TOOL);
        let write = table_call();
        let calls = if checkpoint_first {
            vec![complete.clone(), write.clone()]
        } else {
            vec![write.clone(), complete.clone()]
        };
        assert!(state.admits_tool_batch(11, &calls));
        assert_eq!(state.budgeted_call_count(&calls), 1);
        state.record_calls(&calls);
        assert_eq!(state.total_calls, MAX_CHAT_TOOL_CALLS);
        assert!(stage_completion_boundary(&calls).is_ok());
        validated_chat_tool_request(&complete, &workspaces())?;
        if checkpoint_first {
            state.plan_policy.complete_advisory_stage(&complete)?;
        }
        let mut result =
            completed_tool_exchange(&write.call_id, ipc_types::ToolName::FilesystemWriteText);
        result.request = validated_chat_tool_request(&write, &workspaces())?;
        state
            .plan_policy
            .validate_calls(std::slice::from_ref(&write))?;
        state.record_results(&[write], &[Ok(result)]);
        if !checkpoint_first {
            state.plan_policy.complete_advisory_stage(&complete)?;
        }
        assert!(state.plan_policy.planned_outcome_satisfied());
        assert!(state.plan_policy.forecast_complete());
        assert!(!state.plan_policy.final_response_ready());
        let mut final_round = ChatGenerationRound {
            reasoning: String::new(),
            content: "Created the requested table.".to_owned(),
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
        // No additional effect or inference round is unlocked by the exemption.
        assert!(!state.admits_tool_batch(11, &[call("filesystem.write_text")]));
        assert!(!state.admits_tool_batch(12, &[complete]));
    }
    Ok(())
}

#[test]
fn checkpoint_budget_exemption_does_not_bypass_write_permission() -> Result<(), IpcError> {
    let mut state = active_state()?;
    state.record_calls(&vec![call("filesystem.read_text"); 17]);
    let write = table_call();
    assert!(state.admits_tool_batch(11, &[call(COMPLETE_STAGE_TOOL), write.clone()]));
    let mut read_only = workspaces();
    read_only[0].permission_level = ipc_types::WorkspacePermissionLevel::ReadOnly;
    assert!(validated_chat_tool_request(&write, &read_only).is_err());
    assert!(!state.plan_policy.planned_outcome_satisfied());
    Ok(())
}

#[test]
fn checkpoint_exemption_does_not_admit_extra_tasks_or_invalid_control_batches()
-> Result<(), IpcError> {
    let mut state = active_state()?;
    state.record_calls(&vec![call("filesystem.read_text"); 17]);
    let complete = call(COMPLETE_STAGE_TOOL);
    assert!(!state.admits_tool_batch(
        11,
        &[
            complete.clone(),
            call("filesystem.write_text"),
            call("filesystem.read_text"),
        ]
    ));
    for calls in [
        vec![complete.clone(), complete.clone()],
        vec![complete.clone(), call(SEARCH_TOOLS_TOOL)],
        vec![complete.clone(), call("orchestration.submit_plan")],
    ] {
        assert_eq!(state.budgeted_call_count(&calls), calls.len());
        assert!(!state.admits_tool_batch(11, &calls));
    }
    let mut malformed = complete.clone();
    malformed
        .arguments
        .insert("stageId".to_owned(), serde_json::json!("invented"));
    assert_eq!(state.budgeted_call_count(&[malformed]), 1);
    state.record_calls(std::slice::from_ref(&complete));
    assert_eq!(state.total_calls, 17);
    assert!(!state.plan_policy.planned_outcome_satisfied());
    assert_eq!(state.plan_policy.current_stage_id(), Some("extract"));
    let legacy = GenerationToolState::default();
    assert_eq!(legacy.budgeted_call_count(&[complete]), 1);
    Ok(())
}
