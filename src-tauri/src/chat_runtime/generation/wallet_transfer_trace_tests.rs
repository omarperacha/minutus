//! Transfer-specific diagnostic projection must not copy financial intent.
use super::*;
#[test]
fn wallet_trace_retains_field_names_without_values_or_injected_fields()
-> Result<(), serde_json::Error> {
    let call = ParsedToolCall {
        call_id: "test-call".into(),
        recipient: "wallet.request_transfer".into(),
        name: "wallet.request_transfer".into(),
        arguments: serde_json::from_value(
            serde_json::json!({"amount":"123.456","recipient":"0x1111111111111111111111111111111111111111","reason":"private transfer purpose","signature":"injected secret"}),
        )?,
    };
    let trace = diagnostic_tool_arguments(&call).to_string();
    for private in [
        "123.456",
        "0x1111111111111111111111111111111111111111",
        "private transfer purpose",
        "injected secret",
    ] {
        assert!(!trace.contains(private));
    }
    assert!(trace.contains("fieldNames"));
    Ok(())
}
