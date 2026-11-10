use super::execution::check_cancelled;
use crate::chat_tools::{
    ToolActionClass, ToolCategory, catalog, execution::prepare_chat_tool,
    operation_ledger::OperationLedger, validation::validated_chat_tool_request,
};
use ipc_types::{IpcError, IpcErrorCode, ToolOutcome};
use model_runtime::ParsedToolCall;
use serde_json::json;
use std::sync::atomic::AtomicBool;

fn call(name: &str) -> ParsedToolCall {
    ParsedToolCall {call_id:"contract-call".into(),recipient:name.into(),name:name.into(),arguments:serde_json::from_value(if name == "contract.inspect" {
        json!({"network":"baseSepolia","target":"0x1111111111111111111111111111111111111111","block":"latest"})
    } else {
        json!({"network":"baseSepolia","target":"0x1111111111111111111111111111111111111111","block":"latest","caller":null,"function":"allowance(address,address)","arguments":["0x1111111111111111111111111111111111111111","0x2222222222222222222222222222222222222222"],"outputs":["uint256"]})
    }).unwrap_or_default()}
}

#[test]
fn contracts_need_no_workspace_or_wallet_and_only_read_is_plan_gated() -> Result<(), IpcError> {
    let definitions = crate::chat_tools::chat_tool_definitions(&[])?;
    for name in ["contract.inspect", "contract.read"] {
        let request = call(name);
        validated_chat_tool_request(&request, &[])?;
        assert!(prepare_chat_tool(&[], &request)?.workspace.is_none());
        assert_eq!(
            catalog::tool_action_class(name),
            Some(ToolActionClass::Read)
        );
        assert_eq!(catalog::tool_category(name), Some(ToolCategory::Contracts));
        assert!(
            catalog::tool_category_index(&definitions)
                .iter()
                .any(|category| category.name() == "contracts"
                    && category.operations().contains(&name.to_owned()))
        );
        assert!(crate::chat_tools::is_forecastable_chat_tool(name));
    }
    assert!(
        !definitions
            .iter()
            .any(|tool| tool.name == "contract.write" || tool.name == "contract.request_write")
    );
    Ok(())
}

#[test]
fn contract_intents_reject_authority_fields_and_unsupported_layouts() {
    for field in [
        "rpcUrl",
        "nonce",
        "gas",
        "value",
        "calldata",
        "signature",
        "approved",
        "password",
        "wallet",
    ] {
        let mut request = call("contract.read");
        request.arguments.insert(field.into(), json!("untrusted"));
        assert!(
            validated_chat_tool_request(&request, &[]).is_err(),
            "{field}"
        );
    }
    for (field, value) in [
        ("function", json!("approve(address,uint)")),
        ("function", json!("doWork(string)")),
        ("block", json!("pending")),
        ("outputs", json!(["function"])),
        ("caller", json!("alice.eth")),
        ("network", json!("polygonMainnet")),
    ] {
        let mut request = call("contract.read");
        request.arguments.insert(field.into(), value);
        assert!(
            validated_chat_tool_request(&request, &[]).is_err(),
            "{field}"
        );
    }
}

#[test]
fn cancellation_precedes_endpoint_resolution_and_latest_or_historical_reads_never_reuse()
-> Result<(), IpcError> {
    let cancelled = AtomicBool::new(true);
    for name in ["contract.inspect", "contract.read"] {
        let mut request = call(name);
        assert_eq!(
            check_cancelled(&cancelled).err().map(|error| error.code),
            Some(IpcErrorCode::Cancelled)
        );
        let mut ledger = OperationLedger::default();
        for block in ["latest".to_owned(), format!("0x{}", "ab".repeat(32))] {
            request.arguments.insert("block".into(), json!(block));
            let prepared = prepare_chat_tool(&[], &request)?;
            let exchange = crate::chat_tools::ChatToolExchange {
                request: prepared.request.clone(),
                result: ipc_types::ToolResult {
                    call_id: prepared.request.call_id.clone(),
                    outcome: ToolOutcome::Completed { content: vec![] },
                },
                activity: ipc_types::TaskActivity {
                    status: ipc_types::TaskActivityStatus::Completed,
                    ..prepared.activity.clone()
                },
                workspace_grant_id: None,
                undo_changes: vec![],
                inspected_asset_id: None,
                computation_reuse_digest: None,
            };
            ledger.record_results(std::slice::from_ref(&request), &[Ok(exchange)]);
            assert!(ledger.reusable_exchange(&prepared, None).is_none());
        }
    }
    Ok(())
}

#[test]
fn nested_declared_reads_are_validated_without_new_authority() -> Result<(), IpcError> {
    let mut request = call("contract.read");
    request
        .arguments
        .insert("function".into(), json!("f((uint8,string)[])"));
    request.arguments.insert(
        "arguments".into(),
        json!([[["42", "untrusted instruction"]]]),
    );
    request
        .arguments
        .insert("outputs".into(), json!(["(int24,bytes,string[])"]));
    validated_chat_tool_request(&request, &[])?;
    request
        .arguments
        .insert("arguments".into(), json!([[[42, "untrusted instruction"]]]));
    assert!(validated_chat_tool_request(&request, &[]).is_err());
    Ok(())
}

#[test]
fn source_metadata_is_explicit_unsigned_discovery_and_rejects_authority_inputs()
-> Result<(), IpcError> {
    let plain = call("contract.inspect");
    assert!(!super::intent::request(&plain)?.include_source);
    let mut source = plain.clone();
    source.arguments.insert("includeSource".into(), json!(true));
    validated_chat_tool_request(&source, &[])?;
    let request = super::intent::request(&source)?;
    assert!(request.include_source);
    assert!(request.call.is_none());
    assert!(request.caller.is_none());
    assert!(prepare_chat_tool(&[], &source)?.workspace.is_none());
    for field in [
        "sourceUrl",
        "compilerPath",
        "sourceDigest",
        "abi",
        "storageLayout",
        "approved",
        "wallet",
    ] {
        let mut forged = source.clone();
        forged.arguments.insert(field.into(), json!("untrusted"));
        assert!(
            validated_chat_tool_request(&forged, &[]).is_err(),
            "{field}"
        );
    }
    source
        .arguments
        .insert("includeSource".into(), json!("true"));
    assert!(validated_chat_tool_request(&source, &[]).is_err());
    let mut read = call("contract.read");
    read.arguments.insert("includeSource".into(), json!(true));
    assert!(validated_chat_tool_request(&read, &[]).is_err());
    Ok(())
}

#[test]
fn version2_workflow_reaches_qualification_gate_through_closed_tool_registry()
-> Result<(), Box<dyn std::error::Error>> {
    let arguments = json!({
        "contractVersion":2,"network":"ethereumSepolia",
        "contracts":[{"id":"app","address":"0x2222222222222222222222222222222222222222"}],
        "steps":[{"id":"set","contract":"app","function":"store(uint256)",
            "arguments":["7"],"valueWei":"0","dependsOn":[],"prerequisites":[],
            "completion":[]}],"goals":[]
    });
    let parsed: crate::evm_approval::contracts::DeclaredWorkflowRequest =
        serde_json::from_value(arguments.clone())?;
    parsed.validate().map_err(|error| error.message)?;
    let request = ParsedToolCall {
        call_id: "declared-registry".into(),
        recipient: "contract.request_workflow".into(),
        name: "contract.request_workflow".into(),
        arguments: serde_json::from_value(arguments)?,
    };
    // A raw development checkout can lack packaged helpers. Its closed outer
    // envelope must still reach that qualification gate, not reject v2 fields.
    if let Err(error) = validated_chat_tool_request(&request, &[]) {
        assert_eq!(
            error.code,
            IpcErrorCode::PermissionDenied,
            "{}",
            error.message
        );
    }
    for field in ["gas", "nonce", "sourceUrl", "approved"] {
        let mut forged = request.clone();
        forged.arguments.insert(field.into(), json!("untrusted"));
        assert_eq!(
            validated_chat_tool_request(&forged, &[])
                .err()
                .map(|error| error.code),
            Some(IpcErrorCode::InvalidRequest),
            "{field}"
        );
    }
    Ok(())
}
