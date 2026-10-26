//! Read-only qualification of the actual production tool surface, before app startup.

use std::collections::BTreeMap;

use model_runtime::ParsedToolCall;

pub(crate) fn run() -> Result<(), String> {
    if cfg!(debug_assertions) {
        return Err(
            "x402 release qualification requires an optimised release executable".to_owned(),
        );
    }
    let tools = crate::chat_tools::chat_tool_definitions(&[]).map_err(|error| error.message)?;
    if !tools
        .iter()
        .any(|tool| tool.name == "x402.request" && tool.description.contains("Base mainnet"))
    {
        return Err("The production catalogue does not advertise Base mainnet x402".to_owned());
    }
    let mut call = ParsedToolCall {
        call_id: "release-qualification".to_owned(),
        recipient: "x402.request".to_owned(),
        name: "x402.request".to_owned(),
        arguments: BTreeMap::from([
            ("operation".to_owned(), serde_json::json!("purchase_url")),
            (
                "reason".to_owned(),
                serde_json::json!("Validate request admission without execution"),
            ),
            (
                "url".to_owned(),
                serde_json::json!("https://example.com/qualification"),
            ),
        ]),
    };
    crate::chat_tools::validated_chat_tool_request(&call, &[]).map_err(|error| error.message)?;
    call.arguments.insert(
        "url".to_owned(),
        serde_json::json!("http://127.0.0.1:5001/qualification"),
    );
    if crate::chat_tools::validated_chat_tool_request(&call, &[]).is_ok() {
        return Err("The production x402 surface admitted an HTTP loopback request".to_owned());
    }
    println!(
        "x402 release surface: public HTTPS/mainnet advertised; loopback rejected; no requests or payments executed"
    );
    Ok(())
}
