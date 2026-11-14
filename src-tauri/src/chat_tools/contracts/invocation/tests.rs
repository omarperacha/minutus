use super::*;
use crate::chat_tools::{contracts::intent::request, validation::validated_chat_tool_request};
use model_runtime::ParsedToolCall;
use serde_json::json;

fn source(invocation: &Value) -> ParsedToolCall {
    ParsedToolCall {
        call_id: "local-discovery".into(),
        recipient: "contract.inspect".into(),
        name: "contract.inspect".into(),
        arguments: serde_json::from_value(json!({
            "network":"ethereumSepolia", "block":"latest", "includeSource":true,
            "target":"0x2222222222222222222222222222222222222222",
            "invocation":invocation,
        }))
        .unwrap_or_default(),
    }
}

fn context() -> Value {
    json!({"function":"f((uint8,string)[])","arguments":[[["7","data"]]],
        "caller":"0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"})
}

#[test]
fn local_context_defaults_are_canonical_and_never_become_an_outbound_call() -> Result<(), IpcError>
{
    let call = source(&context());
    validated_chat_tool_request(&call, &[])?;
    let parsed = request(&call)?;
    assert!(parsed.call.is_none());
    assert!(parsed.caller.is_none());
    let local = parsed.invocation.ok_or_else(IpcError::internal)?;
    assert_eq!(
        local.intent.account(),
        "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
    assert_eq!(local.origin, local.intent.account());
    assert_eq!(local.intent.value_wei(), "0");
    assert_eq!(local.intent.call().signature(), "f((uint8,string)[])");
    assert_eq!(local.intent.call().arguments(), &[json!([["7", "data"]])]);
    let mut explicit = context();
    explicit["origin"] = json!("0xBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB");
    explicit["valueWei"] = json!("17");
    let local = request(&source(&explicit))?
        .invocation
        .ok_or_else(IpcError::internal)?;
    assert_eq!(local.origin, "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
    assert_eq!(local.intent.value_wei(), "17");
    Ok(())
}

#[test]
fn local_invocation_rejects_misuse_authority_and_unbounded_or_invalid_context() {
    let mut plain = source(&context());
    plain.arguments.insert("includeSource".into(), json!(false));
    assert!(validated_chat_tool_request(&plain, &[]).is_err());
    let mut read = source(&context());
    read.name = "contract.read".into();
    read.recipient = read.name.clone();
    assert!(validated_chat_tool_request(&read, &[]).is_err());
    assert!(validated_chat_tool_request(&source(&Value::Null), &[]).is_err());
    for (field, value) in [
        ("origin", Value::Null),
        ("valueWei", json!("01")),
        (
            "caller",
            json!("0x0000000000000000000000000000000000000000"),
        ),
        ("arguments", json!([[[256, "bad"]]])),
        ("arguments", json!([[["7", "x".repeat(1025)]]])),
        ("function", json!("f(uint)")),
        ("gas", json!(1)),
        ("nonce", json!("0")),
        ("sourceUrl", json!("https://untrusted.example")),
        ("approved", json!(true)),
        ("calldata", json!("0x")),
    ] {
        let mut forged = context();
        forged[field] = value;
        assert!(
            validated_chat_tool_request(&source(&forged), &[]).is_err(),
            "{field}"
        );
    }
}
