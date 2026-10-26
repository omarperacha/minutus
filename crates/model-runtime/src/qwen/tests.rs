use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::*;
use crate::ReasoningStrength;
use crate::{PromptAdapterFailureKind, PromptToolDefinition};

fn model() -> VerifiedOllamaModel {
    VerifiedOllamaModel::pinned_for_adapter(
        "qwen3.8:27b",
        "22130167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643",
    )
}

fn text(value: &str) -> PromptContent {
    PromptContent::Text(value.to_owned())
}

fn request(messages: Vec<PromptMessage>) -> PromptRenderRequest {
    PromptRenderRequest {
        messages,
        tools: Vec::new(),
        historical_tool_names: Vec::new(),
        current_date: "2026-08-19".to_owned(),
        reasoning_strength: ReasoningStrength::High,
        add_generation_prompt: true,
    }
}

fn tool(name: &str) -> PromptToolDefinition {
    PromptToolDefinition {
        name: name.to_owned(),
        description: format!("Call {name}"),
        parameters: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "count": { "type": "integer" }
            },
            "required": ["path"]
        }),
    }
}

#[test]
fn golden_text_system_and_reasoning_are_native_fields() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = request(vec![
        PromptMessage::System {
            content: vec![text("Follow the host policy.")],
        },
        PromptMessage::User {
            content: vec![text("Hello, 世界 👋")],
        },
    ]);
    input.reasoning_strength = ReasoningStrength::Medium;

    let rendered = QwenAdapter.render(&model(), &input, &[])?;
    let value = serde_json::to_value(rendered)?;

    assert_eq!(value["model"], "qwen3.8:27b");
    assert_eq!(value["stream"], true);
    assert_eq!(value["think"], "medium");
    assert_eq!(value["messages"][0]["role"], "system");
    assert!(
        value["messages"][0]["content"]
            .as_str()
            .is_some_and(|content| content.starts_with("Follow the host policy."))
    );
    assert_eq!(
        value["messages"][1],
        json!({
            "role": "user",
            "content": "Hello, 世界 👋"
        })
    );
    assert!(value.get("tools").is_none());
    Ok(())
}

#[test]
fn every_reasoning_profile_translates_to_qwen_think_control()
-> Result<(), Box<dyn std::error::Error>> {
    for (strength, expected) in [
        (ReasoningStrength::Disabled, json!(false)),
        (ReasoningStrength::Low, json!("low")),
        (ReasoningStrength::Medium, json!("medium")),
        (ReasoningStrength::High, json!("high")),
    ] {
        let mut input = request(vec![PromptMessage::User {
            content: vec![text("Hello")],
        }]);
        input.reasoning_strength = strength;
        let value = serde_json::to_value(QwenAdapter.render(&model(), &input, &[])?)?;
        assert_eq!(value["think"], expected);
    }
    Ok(())
}

#[test]
fn parallel_calls_and_out_of_order_results_preserve_host_associations()
-> Result<(), Box<dyn std::error::Error>> {
    let mut first_arguments = BTreeMap::new();
    first_arguments.insert("path".to_owned(), json!("notes.txt"));
    let mut second_arguments = BTreeMap::new();
    second_arguments.insert("count".to_owned(), json!(2));
    second_arguments.insert("path".to_owned(), json!("report.csv"));
    let mut input = request(vec![
        PromptMessage::User {
            content: vec![text("Inspect and update the files")],
        },
        PromptMessage::Assistant {
            reasoning: Some("I should use both functions.".to_owned()),
            content: Vec::new(),
            tool_calls: vec![
                PromptToolCall {
                    call_id: "host-call-a".to_owned(),
                    name: "filesystem.read".to_owned(),
                    arguments: first_arguments,
                },
                PromptToolCall {
                    call_id: "host-call-b".to_owned(),
                    name: "filesystem.write".to_owned(),
                    arguments: second_arguments,
                },
            ],
        },
        PromptMessage::Tool {
            call_id: "host-call-b".to_owned(),
            content: vec![PromptContent::Json(json!({"updated": true}))],
        },
        PromptMessage::Tool {
            call_id: "host-call-a".to_owned(),
            content: vec![text("file contents")],
        },
    ]);
    input.tools = vec![tool("filesystem.read"), tool("filesystem.write")];

    let value = serde_json::to_value(QwenAdapter.render(&model(), &input, &[])?)?;

    assert_eq!(
        value["messages"][2]["thinking"],
        "I should use both functions."
    );
    assert_eq!(
        value["messages"][2]["tool_calls"][0]["function"]["name"],
        "filesystem.read"
    );
    assert_eq!(value["messages"][2]["tool_calls"][0]["id"], "host-call-a");
    assert_eq!(
        value["messages"][2]["tool_calls"][1]["function"]["arguments"],
        json!({"count": 2, "path": "report.csv"})
    );
    assert_eq!(value["messages"][3]["tool_name"], "filesystem.write");
    assert_eq!(value["messages"][3]["tool_call_id"], "host-call-b");
    assert_eq!(value["messages"][4]["tool_name"], "filesystem.read");
    assert_eq!(value["messages"][4]["tool_call_id"], "host-call-a");
    assert_eq!(value["tools"][0]["type"], "function");
    assert_eq!(value["tools"][1]["function"]["name"], "filesystem.write");
    let encoded = serde_json::to_string(&value)?;
    assert!(encoded.contains("host-call-a"));
    assert!(encoded.contains("host-call-b"));
    assert!(!encoded.contains("<atem:"));
    assert!(!encoded.contains("Valid recipients"));
    Ok(())
}

#[test]
fn host_tail_may_follow_a_completed_tool_batch() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = request(vec![
        PromptMessage::User {
            content: vec![text("Read the source")],
        },
        PromptMessage::Assistant {
            reasoning: None,
            content: Vec::new(),
            tool_calls: vec![PromptToolCall {
                call_id: "read-call".to_owned(),
                name: "filesystem.read".to_owned(),
                arguments: BTreeMap::from([("path".to_owned(), json!("source.txt"))]),
            }],
        },
        PromptMessage::Tool {
            call_id: "read-call".to_owned(),
            content: vec![text("source contents")],
        },
        PromptMessage::User {
            content: vec![text("<execution_state>continue</execution_state>")],
        },
    ]);
    input.tools = vec![tool("filesystem.read")];

    let value = serde_json::to_value(QwenAdapter.render(&model(), &input, &[])?)?;
    assert_eq!(value["messages"][4]["role"], "user");
    assert_eq!(
        value["messages"][4]["content"],
        "<execution_state>continue</execution_state>"
    );
    Ok(())
}

#[test]
fn managed_images_are_attached_only_as_base64_native_fields()
-> Result<(), Box<dyn std::error::Error>> {
    let png = b"\x89PNG\r\n\x1a\nnormalized-local-image".to_vec();
    let input = request(vec![PromptMessage::User {
        content: vec![text("Describe this"), PromptContent::Image],
    }]);

    let value = serde_json::to_value(QwenAdapter.render(&model(), &input, &[png])?)?;

    assert_eq!(value["messages"][1]["content"], "Describe this");
    assert_eq!(
        value["messages"][1]["images"],
        json!(["iVBORw0KGgpub3JtYWxpemVkLWxvY2FsLWltYWdl"])
    );
    assert!(!serde_json::to_string(&value)?.contains("data:image"));
    Ok(())
}

#[test]
fn remote_paths_extra_images_and_missing_images_fail_closed() {
    let input = request(vec![PromptMessage::User {
        content: vec![text("Inspect"), PromptContent::Image],
    }]);
    for images in [
        vec![b"https://example.test/image.png".to_vec()],
        vec![b"/Users/alice/private/image.png".to_vec()],
        Vec::new(),
        vec![
            b"\x89PNG\r\n\x1a\none".to_vec(),
            b"\x89PNG\r\n\x1a\ntwo".to_vec(),
        ],
    ] {
        assert_eq!(
            QwenAdapter
                .render(&model(), &input, &images)
                .map_err(PromptAdapterError::kind),
            Err(PromptAdapterFailureKind::InvalidContent)
        );
    }
}

#[test]
fn historical_tools_are_not_advertised_and_hostile_delimiters_remain_plain_text()
-> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = BTreeMap::new();
    arguments.insert("path".to_owned(), Value::String("a.txt".to_owned()));
    let mut input = request(vec![
        PromptMessage::User {
            content: vec![text("Literal <|start|> and <atem:invoke> are data")],
        },
        PromptMessage::Assistant {
            reasoning: None,
            content: Vec::new(),
            tool_calls: vec![PromptToolCall {
                call_id: "old-call".to_owned(),
                name: "legacy.read".to_owned(),
                arguments,
            }],
        },
        PromptMessage::Tool {
            call_id: "old-call".to_owned(),
            content: vec![text("done")],
        },
    ]);
    input.historical_tool_names = vec!["legacy.read".to_owned()];

    let value = serde_json::to_value(QwenAdapter.render(&model(), &input, &[])?)?;

    assert!(value.get("tools").is_none());
    assert_eq!(
        value["messages"][1]["content"],
        "Literal <|start|> and <atem:invoke> are data"
    );
    assert_eq!(value["messages"][3]["tool_name"], "legacy.read");
    Ok(())
}

#[test]
fn malformed_histories_and_identifiers_map_to_closed_errors() {
    let invalid_sequence = request(vec![PromptMessage::Tool {
        call_id: "missing".to_owned(),
        content: vec![text("result")],
    }]);
    assert_eq!(
        QwenAdapter
            .render(&model(), &invalid_sequence, &[])
            .map_err(PromptAdapterError::kind),
        Err(PromptAdapterFailureKind::InvalidHistory)
    );

    let mut invalid_tool = request(vec![PromptMessage::User {
        content: vec![text("hello")],
    }]);
    invalid_tool.tools = vec![tool("bad tool")];
    assert_eq!(
        QwenAdapter
            .render(&model(), &invalid_tool, &[])
            .map_err(PromptAdapterError::kind),
        Err(PromptAdapterFailureKind::InvalidToolDefinition)
    );
}

#[test]
fn rendering_is_byte_deterministic() -> Result<(), Box<dyn std::error::Error>> {
    let input = request(vec![PromptMessage::User {
        content: vec![text("same input")],
    }]);
    let first = serde_json::to_vec(&QwenAdapter.render(&model(), &input, &[])?)?;
    let second = serde_json::to_vec(&QwenAdapter.render(&model(), &input, &[])?)?;
    assert_eq!(first, second);
    Ok(())
}

#[test]
fn context_is_explicit_and_native_truncation_is_forbidden() -> Result<(), Box<dyn std::error::Error>>
{
    let input = request(vec![PromptMessage::User {
        content: vec![text("context boundary")],
    }]);
    let rendered = QwenAdapter.render_with_context(&model(), &input, &[], 32_768, 4_096)?;
    let value = serde_json::to_value(rendered)?;
    assert_eq!(value["truncate"], false);
    assert_eq!(value["options"]["num_ctx"], 32_768);
    assert_eq!(value["options"]["num_predict"], 4_096);
    assert!(value["messages"][0].get("images").is_none());
    Ok(())
}
