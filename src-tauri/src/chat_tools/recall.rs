//! Closed, read-only conversation recall control.

use ipc_types::IpcError;
use model_runtime::{ParsedToolCall, PromptToolDefinition};

pub(crate) fn tool_definition() -> PromptToolDefinition {
    PromptToolDefinition {
        name: "orchestration.recall".to_owned(),
        description: "Recover a specific needed prior input or historical observation that is missing or truncated in the current context, before asking the user to repeat it. Use already-visible content directly. Do not list or recall history as a routine first step in a fresh conversation or before planning. Call alone. Use reference empty and offset 0 to list a bounded index, then follow nextOffset for further pages. Use an opaque history or retained-result reference to read a UTF-8 byte slice; follow its nextOffset until complete. limit bounds content bytes (1–8192); index pages contain at most 16 entries. Historical recall does not refetch sources, execute stored calls, restore permissions, prove current source truth, or repeat writes/payments. A stale or missing source returns an actionable context gap; use its existing source reader for live evidence. Example: {\"reference\":\"\",\"offset\":0,\"limit\":4096}.".to_owned(),
        parameters: serde_json::json!({
            "type":"object", "properties":{
                "reference":{"type":"string","maxLength":160},
                "offset":{"type":"integer","minimum":0,"maximum":16_777_216},
                "limit":{"type":"integer","minimum":1,"maximum":8192}
            }, "required":["reference","offset","limit"], "additionalProperties":false
        }),
    }
}

pub(crate) fn arguments(call: &ParsedToolCall) -> Result<(&str, usize, usize), IpcError> {
    let invalid = || {
        IpcError::invalid_request(
            "Recall requires reference (opaque string up to 160 bytes), offset (0–16777216), and limit (1–8192). Example: {\"reference\":\"\",\"offset\":0,\"limit\":4096}",
        )
    };
    let reference = call
        .arguments
        .get("reference")
        .and_then(serde_json::Value::as_str)
        .filter(|value| value.len() <= 160)
        .ok_or_else(invalid)?;
    let offset = call
        .arguments
        .get("offset")
        .and_then(serde_json::Value::as_u64)
        .filter(|value| *value <= 16_777_216)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(invalid)?;
    let limit = call
        .arguments
        .get("limit")
        .and_then(serde_json::Value::as_u64)
        .filter(|value| (1..=8192).contains(value))
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(invalid)?;
    Ok((reference, offset, limit))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn recall_is_a_non_forecastable_control_with_closed_bounded_arguments() -> Result<(), String> {
        let mut call = ParsedToolCall {
            call_id: "recall-1".into(),
            name: "orchestration.recall".into(),
            recipient: "orchestration.recall".into(),
            arguments: BTreeMap::from([
                ("reference".into(), serde_json::json!("")),
                ("offset".into(), serde_json::json!(0)),
                ("limit".into(), serde_json::json!(8192)),
            ]),
        };
        let request = crate::chat_tools::validated_chat_tool_request(&call, &[])
            .map_err(|error| error.message)?;
        assert_eq!(request.name, ipc_types::ToolName::OrchestrationRecall);
        assert!(!crate::chat_tools::is_forecastable_chat_tool(&call.name));
        assert!(crate::chat_tools::tool_action_class(&call.name).is_none());
        call.arguments
            .insert("conversationId".into(), serde_json::json!("other"));
        assert!(crate::chat_tools::validated_chat_tool_request(&call, &[]).is_err());
        call.arguments.remove("conversationId");
        for limit in [
            serde_json::json!(0),
            serde_json::json!(8193),
            serde_json::json!(1.5),
        ] {
            call.arguments.insert("limit".into(), limit);
            assert!(crate::chat_tools::validated_chat_tool_request(&call, &[]).is_err());
        }
        Ok(())
    }
}
