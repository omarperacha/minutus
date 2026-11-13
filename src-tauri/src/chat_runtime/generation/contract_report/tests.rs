use super::*;
use ipc_types::{TaskActivity, TaskActivityKind, TaskActivityStatus};
use serde_json::Value;

type TestResult = Result<(), Box<dyn std::error::Error>>;
mod ordering;
fn fixtures() -> Result<Vec<(ParsedToolCall, ChatToolExchange)>, Box<dyn std::error::Error>> {
    let values: Vec<Value> = serde_json::from_str(include_str!(
        "../../../chat_tools/contracts/host_report/public_postsubmit_fixture.json"
    ))?;
    values
        .into_iter()
        .map(|value| {
            let request: ipc_types::ToolRequest = serde_json::from_value(value["request"].clone())?;
            let name = value["request"]["name"]
                .as_str()
                .ok_or("tool name")?
                .to_owned();
            let call = ParsedToolCall {
                call_id: request.call_id.0.clone(),
                recipient: name.clone(),
                name,
                arguments: serde_json::from_value(value["request"]["arguments"].clone())?,
            };
            Ok((
                call,
                ChatToolExchange {
                    request,
                    result: serde_json::from_value(value["result"].clone())?,
                    activity: TaskActivity {
                        id: "historical".into(),
                        title: "Public fixture".into(),
                        kind: TaskActivityKind::TransactionStatus,
                        status: TaskActivityStatus::Partial,
                        detail: None,
                    },
                    workspace_grant_id: None,
                    undo_changes: vec![],
                    inspected_asset_id: None,
                    computation_reuse_digest: None,
                },
            ))
        })
        .collect()
}
fn retained() -> Result<ContractReportState, Box<dyn std::error::Error>> {
    let mut state = ContractReportState::default();
    for (call, exchange) in fixtures()? {
        state.record(1, &[call], &[Ok(exchange)]);
    }
    Ok(state)
}
#[test]
fn request_local_native_failure_and_read_do_not_need_another_synthesis_pass() -> TestResult {
    let state = retained()?;
    let rendered = state.render(1).ok_or("missing complete report")?;
    assert!(rendered.len() < MAX_REPORT_BYTES);
    assert!(rendered.contains("Contract workflow: stopped"));
    assert!(rendered.contains("\"values\":[\"42\"]"));
    assert!(
        state.render(2).is_none(),
        "changed plan revision needs normal synthesis"
    );
    assert!(
        state.binding.is_none(),
        "render alone never admits an output without exact tokenizer binding"
    );
    Ok(())
}
#[test]
fn multiple_workflows_unrelated_results_and_failed_observations_fall_back() -> TestResult {
    let mut state = retained()?;
    let (call, exchange) = fixtures()?.remove(0);
    state.record(1, &[call], &[Ok(exchange)]);
    assert!(state.render(1).is_none());
    let mut state = retained()?;
    let (mut call, exchange) = fixtures()?.remove(1);
    call.name = "web.retrieve".into();
    call.recipient = call.name.clone();
    state.record(1, &[call], &[Ok(exchange)]);
    assert!(state.render(1).is_none());
    let mut state = retained()?;
    let (call, _) = fixtures()?.remove(1);
    state.record(1, &[call], &[Err(IpcError::internal())]);
    assert!(state.render(1).is_none());
    assert!(
        ContractReportState::default().render(1).is_none(),
        "prior requests cannot supply a workflow"
    );
    Ok(())
}
