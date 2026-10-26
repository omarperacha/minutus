use std::collections::BTreeMap;

use super::*;
use crate::chat_tools::{chat_tool_definitions, validation::validated_chat_tool_request};

#[test]
fn computation_output_repair_exposes_all_nested_requirements_without_changing_source()
-> Result<(), IpcError> {
    let catalogue = chat_tool_definitions(&[])?;
    let source = "export default function compute(zeta) { return [4, 6].length; }";
    let mut call = ParsedToolCall {
        call_id: "output-repair".to_owned(),
        recipient: "computation.run".to_owned(),
        name: "computation.run".to_owned(),
        arguments: BTreeMap::from([
            ("source".to_owned(), serde_json::json!(source)),
            ("output".to_owned(), serde_json::json!({"kind": "number"})),
        ]),
    };
    let original = call.arguments.clone();
    let error = validated_chat_tool_request(&call, &[])
        .err()
        .ok_or_else(IpcError::internal)?;
    let repair = exact_argument_repair_error(&call, &catalogue, &error);
    assert!(repair.message.contains("$.output.maxBytes"));
    assert!(repair.message.contains("\"enum\":[\"text\",\"json\"]"));
    assert!(
        repair
            .message
            .contains("\"required\":[\"kind\",\"maxBytes\",\"maxItems\"]")
    );
    assert_eq!(call.arguments, original);

    call.arguments.insert(
        "output".to_owned(),
        serde_json::json!({"kind":"json", "maxBytes":128, "maxItems":1}),
    );
    assert!(validated_chat_tool_request(&call, &[]).is_ok());
    assert_eq!(call.arguments["source"], source);
    Ok(())
}
