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
fn standalone_transfer_is_callable_without_a_plan_but_mixed_batches_are_not() -> Result<(), IpcError>
{
    let mut policy = PlanPolicy::default();
    let all = crate::chat_tools::chat_tool_definitions(&[])?;
    let initial = policy.tools(&all);
    let transfer = initial
        .iter()
        .find(|tool| tool.name == "wallet.request_transfer")
        .ok_or_else(IpcError::internal)?;
    assert!(
        transfer
            .description
            .contains("no semantic plan is required")
    );
    assert_eq!(transfer.parameters["additionalProperties"], false);
    let request = operation("direct-transfer", "wallet.request_transfer");
    assert!(!policy.initial_plan_required(std::slice::from_ref(&request)));
    policy.validate_calls(std::slice::from_ref(&request))?;
    assert!(!policy.active());
    for mixed in [
        vec![
            request.clone(),
            operation("second", "wallet.request_transfer"),
        ],
        vec![operation("web", "web.retrieve"), request.clone()],
        vec![request.clone(), operation("write", "filesystem.write_text")],
    ] {
        assert!(policy.initial_plan_required(&mixed));
        assert!(policy.validate_calls(&mixed).is_err());
    }
    assert!(
        policy
            .validate_calls(&[operation("write", "filesystem.write_text")])
            .is_err()
    );
    assert!(
        policy
            .validate_calls(&[operation("purchase", "x402.request")])
            .is_err()
    );
    Ok(())
}
