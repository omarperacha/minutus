use super::preparation::ContractWorkflowRequest;
use serde_json::{Value, json};
const ACCOUNT: &str = "0x1111111111111111111111111111111111111111";
fn request() -> Value {
    json!({"network":"ethereumSepolia","steps":[{"target":"0x2222222222222222222222222222222222222222","function":"approve(address,uint256)","arguments":[ACCOUNT,"7"],"valueWei":"0","deploymentTransaction":format!("0x{}","a".repeat(64))},{"target":"0x2222222222222222222222222222222222222222","function":"transferFrom(address,address,uint256)","arguments":[ACCOUNT,"0x3333333333333333333333333333333333333333","7"],"valueWei":"0","deploymentTransaction":format!("0x{}","a".repeat(64))}]})
}
fn parse(value: Value) -> Result<ContractWorkflowRequest, serde_json::Error> {
    serde_json::from_value(value)
}
#[test]
fn fixed_intents_support_single_write_and_real_dependent_pull_fixture()
-> Result<(), Box<dyn std::error::Error>> {
    let mut value = request();
    let parsed = parse(value.clone())?;
    parsed.validate().map_err(|error| error.message)?;
    let scope = parsed.scope(ACCOUNT).map_err(|error| error.message)?;
    assert_eq!(
        scope.intent().steps[0].operation,
        "approve(address,uint256)"
    );
    assert_eq!(
        scope.intent().steps[1].operation,
        "transferFrom(address,address,uint256)"
    );
    value["steps"].as_array_mut().ok_or("steps")?.truncate(1);
    parse(value)?.validate().map_err(|error| error.message)?;
    Ok(())
}
#[test]
fn every_static_step_is_checked_before_any_source_or_wallet_work()
-> Result<(), Box<dyn std::error::Error>> {
    for (field, value) in [
        ("function", json!("swap(address,uint256)")),
        ("valueWei", json!("1")),
        ("deploymentTransaction", json!("0x1234")),
    ] {
        let mut bad = request();
        bad["steps"][1][field] = value;
        assert!(parse(bad)?.validate().is_err());
    }
    let mut bad = request();
    bad["steps"][1]["arguments"][1] = json!("0x0000000000000000000000000000000000000000");
    assert!(parse(bad)?.validate().is_err());
    let mut bad = request();
    bad["steps"][1]["deploymentTransaction"] = json!(format!("0x{}", "b".repeat(64)));
    assert!(parse(bad)?.validate().is_err());
    let mut bad = request();
    bad["steps"][0]["profileVerified"] = json!(true);
    assert!(parse(bad).is_err());
    let mut bad = request();
    bad["steps"] = json!(vec![request()["steps"][0].clone(); 17]);
    assert!(parse(bad)?.validate().is_err());
    Ok(())
}
#[test]
fn source_qualified_tool_is_an_external_write_and_terminal_failure_cannot_invite_retry() {
    assert_eq!(
        crate::chat_tools::tool_action_class("contract.request_workflow"),
        Some(crate::chat_tools::ToolActionClass::ExternalWrite)
    );
    let outcome = ipc_types::ToolOutcome::Failed {
        error: ipc_types::IpcError::invalid_request("later step denied; allowance may remain"),
    };
    assert!(crate::chat_tools::terminal_result_completes_stage(
        "contract.request_workflow",
        &std::collections::BTreeMap::new(),
        &outcome
    ));
    assert!(!super::available());
    assert_eq!(
        serde_json::from_value::<ipc_types::ToolName>(json!("contract.request_workflow")).ok(),
        Some(ipc_types::ToolName::ContractRequestWorkflow)
    );
    for alias in [
        "contract.request_write",
        "contract.approve_and_transfer",
        "contract.requestWorkflow",
    ] {
        assert!(serde_json::from_value::<ipc_types::ToolName>(json!(alias)).is_err());
    }
}
