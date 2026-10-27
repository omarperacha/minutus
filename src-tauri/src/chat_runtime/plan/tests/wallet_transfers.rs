//! Scripted host trajectories; these do not claim live-model or native acceptance.
use super::*;
fn transfer_plan() -> ParsedToolCall {
    ParsedToolCall {call_id:"plan-transfer".into(),recipient:PLAN_TOOL.into(),name:PLAN_TOOL.into(),arguments:serde_json::from_value(serde_json::json!({
        "contractVersion":crate::chat_tools::SEMANTIC_PLAN_CONTRACT_VERSION,"mode":"initial","outcome":"other",
        "target":"Request the user's exact ETH transfer","constraints":["No automatic retry"],"assumptions":[],
        "successCriteria":["Report the single native-reviewed attempt"],
        "steps":[{"id":"transfer","actionClass":"external_write","dependsOn":[],"doneWhen":"The single transfer attempt has a reported result","objective":"Request the explicit transfer once","suggestedTools":["wallet.request_transfer"]}]
    })).unwrap_or_default()}
}
#[test]
fn explicit_transfer_is_an_external_effect_and_refusal_ends_the_stage() -> Result<(), IpcError> {
    let all = vec![
        tool(PLAN_TOOL),
        tool(COMPLETE_STAGE_TOOL),
        tool("wallet.request_transfer"),
    ];
    let mut policy = PlanPolicy::default();
    policy.accept(&transfer_plan(), &all, 8)?;
    assert!(
        policy
            .tools(&all)
            .iter()
            .any(|tool| tool.name == "wallet.request_transfer")
    );
    let request = operation("request-transfer", "wallet.request_transfer");
    policy.validate_calls(std::slice::from_ref(&request))?;
    let mut denial = completed_operation("request-transfer", ToolName::WalletRequestTransfer);
    denial.result.outcome = ToolOutcome::Completed {
        content: vec![ipc_types::ToolResultPart::Json {
            value: JsonValue::Object(BTreeMap::from([(
                "state".into(),
                JsonValue::String("denied".into()),
            )])),
        }],
    };
    policy.record_results(&[request], &[Ok(denial)]);
    assert!(policy.final_response_ready());
    assert!(policy.tools(&all).is_empty());
    Ok(())
}
#[test]
fn a_model_cannot_invoke_transfer_before_external_effect_planning() {
    let policy = PlanPolicy::default();
    let all = vec![tool(PLAN_TOOL), tool("wallet.request_transfer")];
    assert!(
        !policy
            .tools(&all)
            .iter()
            .any(|tool| tool.name == "wallet.request_transfer")
    );
}
