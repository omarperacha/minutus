use super::*;
use crate::evm_approval::ApprovalOrigin;
use ipc_types::ToolOutcome;
use model_runtime::ParsedToolCall;
use std::collections::BTreeMap;
fn call(value: &serde_json::Value) -> ParsedToolCall {
    ParsedToolCall {
        call_id: "transfer-call".into(),
        recipient: "wallet.request_transfer".into(),
        name: "wallet.request_transfer".into(),
        arguments: value
            .as_object()
            .map(|fields| fields.clone().into_iter().collect())
            .unwrap_or_default(),
    }
}
fn valid() -> serde_json::Value {
    serde_json::json!({"network":"baseSepolia","asset":"ETH","recipient":"0x1111111111111111111111111111111111111111","amount":"0.001"})
}
#[test]
fn all_eight_network_asset_pairs_validate_without_wallet_or_network() {
    for network in [
        "baseMainnet",
        "baseSepolia",
        "ethereumMainnet",
        "ethereumSepolia",
    ] {
        for asset in ["ETH", "USDC"] {
            let mut value = valid();
            value["network"] = network.into();
            value["asset"] = asset.into();
            assert!(validate_call(&call(&value)).is_ok());
        }
    }
}
#[test]
fn malformed_intents_cannot_reach_wallet_or_rpc() {
    for (field, value) in [
        ("network", serde_json::json!("ethereum")),
        ("network", serde_json::Value::Null),
        ("asset", serde_json::json!("DAI")),
        ("amount", serde_json::json!(0.001)),
        ("recipient", serde_json::json!("example.eth")),
        (
            "recipient",
            serde_json::json!("0x0000000000000000000000000000000000000000"),
        ),
        ("reason", serde_json::json!("\u{202e}trusted")),
    ] {
        let mut request = valid();
        request[field] = value;
        assert!(validate_call(&call(&request)).is_err(), "{field}");
    }
    for amount in ["0", "-1", "+1", "1e3", "all", "1.0000000000000000001"] {
        let mut request = valid();
        request["amount"] = amount.into();
        assert!(validate_call(&call(&request)).is_err(), "{amount}");
    }
    for field in [
        "sender",
        "nonce",
        "gas",
        "calldata",
        "rpcUrl",
        "approved",
        "signature",
    ] {
        let mut request = valid();
        request[field] = "injected".into();
        assert!(validate_call(&call(&request)).is_err(), "{field}");
    }
    let mut request = valid();
    request["asset"] = "USDC".into();
    request["amount"] = "0.0000001".into();
    assert!(validate_call(&call(&request)).is_err());
}
#[test]
fn transfer_contract_is_independent_of_project_folders_and_terminates_after_denial()
-> Result<(), ipc_types::IpcError> {
    let request = super::super::validation::validated_chat_tool_request(&call(&valid()), &[])?;
    assert_eq!(request.name, ipc_types::ToolName::WalletRequestTransfer);
    assert_eq!(
        super::super::tool_action_class("wallet.request_transfer"),
        Some(super::super::ToolActionClass::ExternalWrite)
    );
    let denied = ToolOutcome::Completed { content: vec![] };
    assert!(super::super::terminal_result_completes_stage(
        "wallet.request_transfer",
        &BTreeMap::new(),
        &denied
    ));
    let display = ApprovalOrigin::Chat {
        conversation: [1; 32],
        call: [2; 32],
    };
    assert_ne!(display, ApprovalOrigin::User);
    Ok(())
}

#[test]
fn emitted_amount_schema_accepts_decimal_strings_and_the_example_is_plain_json()
-> Result<(), Box<dyn std::error::Error>> {
    let definition = tool_definition();
    let pattern = definition.parameters["properties"]["amount"]["pattern"]
        .as_str()
        .ok_or("missing pattern")?;
    let regex = regex::Regex::new(pattern)?;
    assert!(regex.is_match("0.001"));
    for invalid in ["1e3", "-1", "all", "1\\2"] {
        assert!(!regex.is_match(invalid));
    }
    assert!(
        definition
            .description
            .contains(r#"{"network":"baseSepolia""#)
    );
    let mut missing = valid();
    missing.as_object_mut().ok_or("object")?.remove("network");
    assert!(validate_call(&call(&missing)).is_err());
    Ok(())
}
