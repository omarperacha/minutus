//! Conversation-scoped preservation and resolution of x402 workflow resources.

use std::collections::{BTreeMap, BTreeSet};

use super::transport::Scope;
use app_core::ToolTaskReference;
use ipc_types::{
    Conversation, JsonValue, MessagePart, MessageRole, ToolName, ToolOutcome, ToolResultPart,
};

use super::{bazaar::BazaarResource, configured_scope};

pub(crate) fn active_task_references(conversation: &Conversation) -> Vec<ToolTaskReference> {
    let Ok(scope) = configured_scope() else {
        return Vec::new();
    };
    let consumed = completed_purchase_references(conversation);
    let discovered = discovered_resources_in_scope(conversation, &scope);
    let has_discovery = !discovered.is_empty();
    let mut references = discovered
        .into_iter()
        .rev()
        .filter(|resource| !consumed.contains(resource.reference()))
        .filter_map(|resource| {
            ToolTaskReference::x402_resource(
                resource.reference().to_owned(),
                resource.source_call_id().to_owned(),
            )
        })
        .collect::<Vec<_>>();

    if !has_discovery
        && let Some(url) = recent_operator_url(conversation, &scope)
        && let Some(reference) = ToolTaskReference::user_supplied_url(url)
    {
        references.push(reference);
    }
    references
}

pub(super) fn resolve_resource(
    conversation: &Conversation,
    reference: &str,
    scope: &Scope,
) -> Option<BazaarResource> {
    discovered_resources_in_scope(conversation, scope)
        .into_iter()
        .rev()
        .find(|resource| resource.reference() == reference)
}

fn discovered_resources_in_scope(
    conversation: &Conversation,
    scope: &Scope,
) -> Vec<BazaarResource> {
    let discovery_calls = x402_calls(conversation)
        .into_iter()
        .filter(|(_, operation, _)| operation == "discover")
        .map(|(call_id, _, _)| call_id)
        .collect::<BTreeSet<_>>();

    conversation
        .messages
        .iter()
        .flat_map(|message| &message.parts)
        .filter_map(|part| {
            let MessagePart::ToolResult { result } = part else {
                return None;
            };
            let ToolOutcome::Completed { content } = &result.outcome else {
                return None;
            };
            discovery_calls
                .contains(result.call_id.0.as_str())
                .then_some((result.call_id.0.as_str(), content))
        })
        .flat_map(|(call_id, content)| {
            content.iter().flat_map(move |part| {
                let ToolResultPart::Json { value } = part else {
                    return Vec::new().into_iter();
                };
                let Ok(value) = serde_json::to_value(value) else {
                    return Vec::new().into_iter();
                };
                value
                    .get("resources")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|resource| {
                        BazaarResource::from_persisted(resource, call_id, scope).ok()
                    })
                    .collect::<Vec<_>>()
                    .into_iter()
            })
        })
        .collect()
}

/// A resource is consumed only by a completed purchase result. Merely emitting
/// a malformed, unavailable, rejected, or cancelled call cannot destroy the
/// continuation needed to repair that call.
fn completed_purchase_references(conversation: &Conversation) -> BTreeSet<String> {
    let completed_call_ids = conversation
        .messages
        .iter()
        .flat_map(|message| &message.parts)
        .filter_map(|part| {
            let MessagePart::ToolResult { result } = part else {
                return None;
            };
            matches!(result.outcome, ToolOutcome::Completed { .. })
                .then(|| result.call_id.0.clone())
        })
        .collect::<BTreeSet<_>>();

    x402_calls(conversation)
        .into_iter()
        .filter(|(call_id, operation, _)| {
            operation == "purchase" && completed_call_ids.contains(call_id)
        })
        .filter_map(|(_, _, arguments)| ipc_string(arguments.get("resourceRef")).map(str::to_owned))
        .collect()
}

fn x402_calls(conversation: &Conversation) -> Vec<(String, String, &BTreeMap<String, JsonValue>)> {
    conversation
        .messages
        .iter()
        .flat_map(|message| &message.parts)
        .filter_map(|part| {
            let MessagePart::ToolCall { request } = part else {
                return None;
            };
            if request.name != ToolName::X402Request {
                return None;
            }
            Some((
                request.call_id.0.clone(),
                ipc_string(request.arguments.get("operation"))?.to_owned(),
                &request.arguments,
            ))
        })
        .collect()
}

fn recent_operator_url(conversation: &Conversation, scope: &Scope) -> Option<String> {
    conversation
        .messages
        .iter()
        .rev()
        .filter(|message| message.role == MessageRole::User)
        .take(4)
        .find_map(|message| {
            message.parts.iter().rev().find_map(|part| {
                let MessagePart::Text { text } = part else {
                    return None;
                };
                x402_url_in_text(text, scope)
            })
        })
}

fn x402_url_in_text(text: &str, scope: &Scope) -> Option<String> {
    text.match_indices("http://").find_map(|(start, _)| {
        let remainder = &text[start..];
        let end = remainder
            .char_indices()
            .skip(1)
            .find(|(_, character)| {
                character.is_whitespace() || matches!(character, ')' | ']' | '>' | '"' | '\'' | ',')
            })
            .map_or(remainder.len(), |(index, _)| index);
        let candidate = remainder[..end].trim_end_matches('.');
        scope.admits(candidate).then(|| candidate.to_owned())
    })
}

fn ipc_string(value: Option<&JsonValue>) -> Option<&str> {
    let JsonValue::String(value) = value? else {
        return None;
    };
    Some(value)
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::*;
    use ipc_types::{
        ConversationId, IpcError, Message, MessageId, ProjectId, ToolCallId, ToolRequest,
        ToolResult,
    };

    const RESOURCE_URL: &str =
        "http://127.0.0.1:5001/test-project/us-central1/api/x402/widgets/purchase";

    fn resource_ref() -> String {
        super::super::bazaar::resource_reference("discovery-call", RESOURCE_URL)
    }

    fn ipc(value: serde_json::Value) -> Result<JsonValue, String> {
        serde_json::from_value(value).map_err(|error| error.to_string())
    }

    fn discovery_result() -> Result<JsonValue, String> {
        ipc(serde_json::json!({
            "operation":"discovery",
            "resources":[{
                "resourceRef":resource_ref(),
                "resource":RESOURCE_URL,
                "method":"POST",
                "bodyExample":{"widget":"blue"},
                "inputSchema":{
                    "type":"object",
                    "additionalProperties":false,
                    "required":["widget"],
                    "properties":{"widget":{"type":"string"}}
                },
                "output":null,
                "hostGeneratedFields":["requestId","accessToken"]
            }]
        }))
    }

    fn message(id: &str, role: MessageRole, parts: Vec<MessagePart>) -> Message {
        Message {
            id: MessageId(id.to_owned()),
            conversation_id: ConversationId("conversation".to_owned()),
            role,
            parts,
            created_at: "2026-09-10T00:00:00Z".to_owned(),
        }
    }

    fn conversation(purchase_outcome: Option<ToolOutcome>) -> Result<Conversation, String> {
        let discovery_call_id = ToolCallId("discovery-call".to_owned());
        let mut messages = vec![
            message(
                "discover-call",
                MessageRole::Assistant,
                vec![MessagePart::ToolCall {
                    request: ToolRequest {
                        call_id: discovery_call_id.clone(),
                        name: ToolName::X402Request,
                        arguments: BTreeMap::from([
                            ("operation".to_owned(), JsonValue::String("discover".to_owned())),
                            ("reason".to_owned(), JsonValue::String("test".to_owned())),
                            (
                                "url".to_owned(),
                                JsonValue::String(
                                    "http://127.0.0.1:5001/test-project/us-central1/api/x402/discovery/resources".to_owned(),
                                ),
                            ),
                        ]),
                    },
                }],
            ),
            message(
                "discover-result",
                MessageRole::Tool,
                vec![MessagePart::ToolResult {
                    result: ToolResult {
                        call_id: discovery_call_id,
                        outcome: ToolOutcome::Completed {
                            content: vec![ToolResultPart::Json {
                                value: discovery_result()?,
                            }],
                        },
                    },
                }],
            ),
        ];
        if let Some(outcome) = purchase_outcome {
            let purchase_call_id = ToolCallId("purchase-call".to_owned());
            messages.extend([
                message(
                    "purchase-call",
                    MessageRole::Assistant,
                    vec![MessagePart::ToolCall {
                        request: ToolRequest {
                            call_id: purchase_call_id.clone(),
                            name: ToolName::X402Request,
                            arguments: BTreeMap::from([
                                (
                                    "operation".to_owned(),
                                    JsonValue::String("purchase".to_owned()),
                                ),
                                ("resourceRef".to_owned(), JsonValue::String(resource_ref())),
                                (
                                    "input".to_owned(),
                                    ipc(serde_json::json!({"widget":"blue"}))?,
                                ),
                                ("reason".to_owned(), JsonValue::String("test".to_owned())),
                            ]),
                        },
                    }],
                ),
                message(
                    "purchase-result",
                    MessageRole::Tool,
                    vec![MessagePart::ToolResult {
                        result: ToolResult {
                            call_id: purchase_call_id,
                            outcome,
                        },
                    }],
                ),
            ]);
        }
        Ok(Conversation {
            id: ConversationId("conversation".to_owned()),
            project_id: ProjectId("project".to_owned()),
            title: "x402".to_owned(),
            messages,
            created_at: "2026-09-10T00:00:00Z".to_owned(),
            updated_at: "2026-09-10T00:00:00Z".to_owned(),
        })
    }

    #[test]
    fn rejected_purchase_keeps_resource_but_completed_purchase_consumes_it() -> Result<(), String> {
        let failed = ToolOutcome::Failed {
            error: IpcError::invalid_request("missing field"),
        };
        assert_eq!(
            active_task_references(&conversation(Some(failed))?).len(),
            1
        );
        let completed = ToolOutcome::Completed {
            content: Vec::new(),
        };
        assert!(active_task_references(&conversation(Some(completed))?).is_empty());
        Ok(())
    }

    #[test]
    fn resource_resolution_rebuilds_only_the_trusted_discovery_projection() -> Result<(), String> {
        let scope = Scope::parse(super::super::tests::BASE_URL).map_err(|_| "scope")?;
        let conversation = conversation(None)?;
        let reference = resource_ref();
        let resource = resolve_resource(&conversation, &reference, &scope).ok_or("resource")?;
        assert_eq!(resource.url(), RESOURCE_URL);
        assert_eq!(resource.source_call_id(), "discovery-call");
        Ok(())
    }
}
