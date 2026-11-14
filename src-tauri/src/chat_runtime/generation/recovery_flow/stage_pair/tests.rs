//! Replay the real rejected stage/workflow batch without RPC or wallet access.
use super::*;
use crate::chat_runtime::generation::{ChatTerminal, ToolOutcome};
use crate::chat_tools::{
    ChatToolExchange, chat_tool_definitions, declared_workflow_schema_for_tests,
    exact_argument_repair_error,
};
use ipc_types::{TaskActivityStatus, ToolResult};
use serde_json::{Value, json};
use std::sync::Arc;

fn fixture() -> Result<(ParsedToolCall, Vec<ParsedToolCall>), Box<dyn std::error::Error>> {
    let data: Value =
        serde_json::from_str(include_str!("tests/fixtures/sepolia-stage-workflow.json"))?;
    let plan = ParsedToolCall {
        call_id: "plan".into(),
        recipient: "orchestration.submit_plan".into(),
        name: "orchestration.submit_plan".into(),
        arguments: serde_json::from_value(data["plan"].clone())?,
    };
    let calls = data["calls"]
        .as_array()
        .ok_or("fixture calls")?
        .iter()
        .enumerate()
        .map(|(i, x)| {
            Ok(ParsedToolCall {
                call_id: format!("pair-{i}"),
                name: x["name"].as_str().ok_or("tool")?.into(),
                recipient: x["recipient"].as_str().ok_or("recipient")?.into(),
                arguments: serde_json::from_value(x["arguments"].clone())?,
            })
        })
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
    Ok((plan, calls))
}
struct RepairFixture {
    state: GenerationToolState,
    tools: Vec<PromptToolDefinition>,
    calls: Vec<ParsedToolCall>,
    error: IpcError,
}
fn setup() -> Result<RepairFixture, Box<dyn std::error::Error>> {
    let (plan, calls) = fixture()?;
    let mut tools = chat_tool_definitions(&[]).map_err(|error| error.message)?;
    tools.retain(|tool| tool.name != "contract.request_workflow");
    // The real schema is metadata only; native worker/release admission is never overridden.
    tools.push(declared_workflow_schema_for_tests());
    let mut state = GenerationToolState::default();
    state
        .plan_policy
        .accept(&plan, &tools, 20)
        .map_err(|error| error.message)?;
    let data: Value =
        serde_json::from_str(include_str!("tests/fixtures/sepolia-stage-workflow.json"))?;
    let request: ipc_types::ToolRequest =
        serde_json::from_value(data["priorRead"]["request"].clone())?;
    let result: ToolResult = serde_json::from_value(data["priorRead"]["result"].clone())?;
    let read = ParsedToolCall {
        call_id: "prior-read".into(),
        recipient: "contract.read".into(),
        name: "contract.read".into(),
        arguments: serde_json::from_value(json!(request.arguments))?,
    };
    let exchange = ChatToolExchange {
        request,
        result,
        activity: ipc_types::TaskActivity {
            status: TaskActivityStatus::Completed,
            ..crate::chat_runtime::tool_activity(&read.call_id, &read.name)
        },
        workspace_grant_id: None,
        undo_changes: vec![],
        inspected_asset_id: None,
        computation_reuse_digest: None,
    };
    state.record_results(&[read], &[Ok(exchange)]);
    state.observe_recovery_checkpoint();
    let error = exact_argument_repair_error(
        &calls[1],
        &tools,
        &IpcError::invalid_request("Use the exact version 2 declared-workflow fields"),
    );
    Ok(RepairFixture {
        state,
        tools,
        calls,
        error,
    })
}
fn attempt(
    state: &mut GenerationToolState,
    calls: &[ParsedToolCall],
    tools: &[PromptToolDefinition],
    error: &IpcError,
) -> Result<Option<crate::chat_tools::RejectedStageOperationPair>, ChatTerminal> {
    prepare(state, calls, &[(1, error.clone())], tools, error)
}

#[test]
fn real_batch_gets_two_failed_receipts_without_progress_or_argument_rewriting()
-> Result<(), Box<dyn std::error::Error>> {
    let RepairFixture {
        mut state,
        tools,
        calls,
        error,
    } = setup()?;
    let before = state.plan_policy.progress_projection(false);
    let revision = state.plan_policy.revision();
    let useful = state.operation_ledger.useful_result_sequence();
    assert!(useful > 0);
    let original = super::super::tool_call_fingerprint(&calls);
    assert!(
        error
            .message
            .contains("$.steps[0].prerequisites[0].comparison")
    );
    assert!(error.message.contains("[\"equal\",\"atLeast\",\"atMost\"]"));
    assert_eq!(state.plan_policy.current_stage_id(), Some("read-current"));
    let pair = attempt(&mut state, &calls, &tools, &error)
        .map_err(|_| "repair was terminal")?
        .ok_or("no repair")?;
    for exchange in pair.exchanges() {
        assert!(matches!(
            exchange.result.outcome,
            ToolOutcome::Failed { .. }
        ));
        assert_eq!(exchange.activity.status, TaskActivityStatus::Partial);
        assert!(exchange.workspace_grant_id.is_none());
        assert!(exchange.undo_changes.is_empty());
    }
    let ToolOutcome::Failed {
        error: control_error,
    } = &pair.exchanges()[0].result.outcome
    else {
        return Err("control completed".into());
    };
    assert!(control_error.message.contains("Neither call was executed"));
    let ToolOutcome::Failed { error: task_error } = &pair.exchanges()[1].result.outcome else {
        return Err("workflow completed".into());
    };
    assert!(
        task_error
            .message
            .contains("$.steps[0].prerequisites[0].comparison")
    );
    assert_eq!(
        pair.exchanges()[1].request.arguments["steps"].clone(),
        crate::chat_tools::ipc_json_value(&calls[1].arguments["steps"])
            .map_err(|error| error.message)?
    );
    assert_eq!(super::super::tool_call_fingerprint(&calls), original);
    assert_eq!(state.plan_policy.progress_projection(false), before);
    assert_eq!(state.plan_policy.revision(), revision);
    assert_eq!(state.operation_ledger.useful_result_sequence(), useful);
    assert_eq!(state.argument_corrections, 1);
    assert!(state.total_calls > 0);
    assert!(state.recovery.pending().is_some());

    // This represents a NEW model decision, never a host normalisation.
    let mut corrected = calls.clone();
    corrected[1].arguments.get_mut("steps").ok_or("steps")?[0]["prerequisites"][0]["comparison"] =
        json!("equal");
    corrected[1].arguments.get_mut("steps").ok_or("steps")?[0]["prerequisites"][0]["expected"] =
        json!("1");
    let request: crate::evm_approval::contracts::DeclaredWorkflowRequest =
        serde_json::from_value(json!(corrected[1].arguments))?;
    request.validate().map_err(|error| error.message)?;
    state
        .plan_policy
        .complete_advisory_stage(&corrected[0])
        .map_err(|error| error.message)?;
    state
        .plan_policy
        .validate_calls(&corrected[1..])
        .map_err(|error| error.message)?;
    assert_eq!(state.plan_policy.current_stage_id(), Some("store-write"));
    Ok(())
}

#[test]
fn unsafe_ambiguous_or_unavailable_pairs_never_enter_correction()
-> Result<(), Box<dyn std::error::Error>> {
    let RepairFixture {
        tools,
        calls,
        error,
        ..
    } = setup()?;
    let mut cases = Vec::new();
    let mut changed = calls.clone();
    changed.swap(0, 1);
    cases.push(changed);
    let mut changed = calls.clone();
    changed[0].recipient = "wrong".into();
    cases.push(changed);
    let mut changed = calls.clone();
    changed[0]
        .arguments
        .insert("stageId".into(), json!("guess"));
    cases.push(changed);
    let mut changed = calls.clone();
    changed[1].recipient = "contract.inspect".into();
    cases.push(changed);
    let mut changed = calls.clone();
    changed[1].name = "unknown.write".into();
    changed[1].recipient = "unknown.write".into();
    cases.push(changed);
    let mut changed = calls.clone();
    changed[1].name = "orchestration.submit_plan".into();
    changed[1].recipient = changed[1].name.clone();
    cases.push(changed);
    let mut changed = calls.clone();
    changed[1].call_id = changed[0].call_id.clone();
    cases.push(changed);
    let mut changed = calls.clone();
    changed.push(changed[1].clone());
    cases.push(changed);
    let mut changed = calls.clone();
    changed[1]
        .arguments
        .insert("oversize".into(), json!("x".repeat(300_000)));
    cases.push(changed);
    for changed in cases {
        let RepairFixture { mut state, .. } = setup()?;
        let before = state.plan_policy.progress_projection(false);
        assert!(
            attempt(&mut state, &changed, &tools, &error)
                .map_err(|_| "unexpected terminal")?
                .is_none()
        );
        assert_eq!(state.argument_corrections, 0);
        assert_eq!(state.plan_policy.progress_projection(false), before);
    }
    let RepairFixture { mut state, .. } = setup()?;
    assert!(
        prepare(
            &mut state,
            &calls,
            &[(0, error.clone()), (1, error.clone())],
            &tools,
            &error
        )
        .map_err(|_| "terminal")?
        .is_none()
    );
    let mut unavailable = tools;
    unavailable.retain(|t| t.name != "contract.request_workflow");
    assert!(
        attempt(&mut state, &calls, &unavailable, &error)
            .map_err(|_| "terminal")?
            .is_none()
    );
    assert!(
        attempt(
            &mut GenerationToolState::default(),
            &calls,
            &unavailable,
            &error
        )
        .map_err(|_| "terminal")?
        .is_none()
    );
    let denied = IpcError::permission_denied("No write permission");
    assert!(
        attempt(&mut state, &calls, &unavailable, &denied)
            .map_err(|_| "terminal")?
            .is_none()
    );
    Ok(())
}

#[test]
fn corrections_keep_existing_retry_and_call_budget_limits() -> Result<(), Box<dyn std::error::Error>>
{
    let RepairFixture {
        mut state,
        tools,
        calls,
        error,
    } = setup()?;
    state.argument_corrections = MAX_ARGUMENT_CORRECTIONS;
    assert!(
        attempt(&mut state, &calls, &tools, &error)
            .map_err(|_| "terminal")?
            .is_none()
    );
    state.argument_corrections = 0;
    for _ in 0..crate::chat_runtime::generation::MAX_RECOVERY_ATTEMPTS {
        assert!(
            attempt(&mut state, &calls, &tools, &error)
                .map_err(|_| "premature terminal")?
                .is_some()
        );
    }
    assert!(attempt(&mut state, &calls, &tools, &error).is_err());
    assert_eq!(state.argument_corrections, 3);
    assert!(state.total_calls >= 3);
    Ok(())
}

#[test]
fn argument_correction_neither_consumes_nor_restores_a_durable_workflow_turn()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let core = Arc::new(app_core::AppCore::initialize(directory.path())?);
    let project = core.create_project("Correction fixture", "2026-11-11T00:00:00Z")?;
    let conversation = core.create_project_conversation(&project.id, "2026-11-11T00:00:00Z")?;
    let conversation = core.append_user_message(
        &conversation.id,
        "Test no-effect schema correction".into(),
        vec![],
        "2026-11-11T00:00:01Z",
    )?;
    let message = conversation.messages.last().ok_or("user turn")?;
    let RepairFixture {
        mut state,
        tools,
        calls,
        error,
    } = setup()?;
    assert!(
        attempt(&mut state, &calls, &tools, &error)
            .map_err(|_| "terminal")?
            .is_some()
    );
    let claim = core
        .claim_contract_workflow_attempt(&conversation.id, &message.id)?
        .ok_or("correction consumed turn")?;
    drop(claim);
    let mut changed = calls;
    changed[1].arguments.insert("badField".into(), json!(true));
    assert!(
        attempt(&mut state, &changed, &tools, &error)
            .map_err(|_| "terminal")?
            .is_some()
    );
    assert!(
        core.claim_contract_workflow_attempt(&conversation.id, &message.id)?
            .is_none()
    );
    Ok(())
}
