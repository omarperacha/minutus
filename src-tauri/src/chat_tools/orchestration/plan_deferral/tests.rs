use super::*;
use std::collections::BTreeMap;

fn call(id: &str, name: &str) -> ParsedToolCall {
    ParsedToolCall {
        call_id: id.into(),
        recipient: name.into(),
        name: name.into(),
        arguments: BTreeMap::from([("path".into(), serde_json::json!("private-original.csv"))]),
    }
}

#[test]
fn known_mixed_batch_is_atomic_inert_and_truthfully_labelled() -> Result<(), IpcError> {
    let calls = vec![
        call("read", "filesystem.read_text"),
        call("write", "filesystem.write_text"),
        call("pay", "x402.request"),
    ];
    let exchanges = deferred_plan_exchanges(&calls)?;
    assert_eq!(exchanges.len(), calls.len());
    for (exchange, call) in exchanges.iter().zip(&calls) {
        assert_eq!(exchange.request.call_id.0, call.call_id);
        assert_eq!(exchange.result.call_id, exchange.request.call_id);
        assert_eq!(exchange.activity.status, TaskActivityStatus::Partial);
        assert!(exchange.activity.title.contains("not executed"));
        assert!(exchange.workspace_grant_id.is_none());
        assert!(exchange.undo_changes.is_empty());
        assert!(exchange.inspected_asset_id.is_none());
        assert!(exchange.computation_reuse_digest.is_none());
        let ToolOutcome::Failed { error } = &exchange.result.outcome else {
            return Err(IpcError::internal());
        };
        assert!(error.message.contains("requiredNextOperation"));
        assert!(error.message.contains("submit_plan"));
        assert!(!error.message.contains("private-original"));
    }
    Ok(())
}

#[test]
fn unsafe_unknown_duplicate_and_control_batches_are_not_deferred() {
    let known = call("safe", "filesystem.read_text");
    let mut unsafe_call = call("unsafe", "filesystem.write_text");
    unsafe_call.recipient = "elsewhere".into();
    for other in [
        unsafe_call,
        call("unknown", "filesystem.imaginary"),
        call("plan", PLAN_REQUIRED_OPERATION),
        known.clone(),
    ] {
        assert!(!plan_deferral_batch_is_bounded(&[known.clone(), other]));
    }
    assert!(!plan_deferral_batch_is_bounded(&[]));
    let mut oversized = known;
    oversized
        .arguments
        .insert("path".into(), serde_json::json!("x".repeat(2_000_000)));
    assert!(deferred_plan_exchanges(&[oversized]).is_err());
}

#[test]
fn deferred_original_arguments_and_prior_results_survive_reopen()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::TempDir::new()?;
    let core = app_core::AppCore::initialize(dir.path().join("Zeta"))?;
    let now = "2026-09-10T12:00:00Z";
    let project = core.create_project("Test", now)?;
    let mut conversation = core.create_project_conversation(&project.id, now)?;
    let request = RequestId("test".into());
    let calls = [call("deferred", "filesystem.write_text")];
    let mut earlier =
        deferred_plan_exchanges(&[call("earlier", "web.search")]).map_err(|error| error.message)?;
    earlier[0].result.outcome = ToolOutcome::Completed {
        content: Vec::new(),
    };
    crate::chat_tools::persist_tool_exchanges(
        &core,
        &request,
        &mut conversation,
        String::new(),
        "Prior answer".into(),
        earlier.into_iter().map(Ok).collect(),
        &[],
        "",
    )
    .map_err(|error| error.message)?;
    crate::chat_tools::persist_tool_exchanges(
        &core,
        &request,
        &mut conversation,
        String::new(),
        String::new(),
        deferred_plan_exchanges(&calls)
            .map_err(|error| error.message)?
            .into_iter()
            .map(Ok)
            .collect(),
        &[],
        "",
    )
    .map_err(|error| error.message)?;
    drop(core);
    let core = app_core::AppCore::initialize(dir.path().join("Zeta"))?;
    let restored = core
        .conversation(&conversation.id)?
        .ok_or("missing history")?;
    let encoded = serde_json::to_string(&restored)?;
    assert!(encoded.contains("private-original.csv"));
    assert!(encoded.contains("Prior answer"));
    assert!(encoded.contains("plan_pending"));
    assert_eq!(restored.messages.iter().flat_map(|message| &message.parts).filter(|part| matches!(part, ipc_types::MessagePart::ToolResult { result } if matches!(result.outcome, ToolOutcome::Completed { .. }))).count(), 1);
    Ok(())
}
