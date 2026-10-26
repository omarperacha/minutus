use std::{collections::BTreeMap, error::Error};

use serde_json::json;

use super::QwenStreamDecoder;
use crate::{GenerationCompletion, GenerationEvent, ModelOutputFailureKind, ParsedToolCall};

fn by_byte(decoder: &mut QwenStreamDecoder, input: &str) -> Vec<GenerationEvent> {
    input
        .as_bytes()
        .iter()
        .flat_map(|byte| decoder.push(&[*byte]))
        .collect()
}

fn terminal_count(events: &[GenerationEvent]) -> usize {
    events
        .iter()
        .filter(|event| {
            matches!(
                event,
                GenerationEvent::Completed { .. }
                    | GenerationEvent::Cancelled
                    | GenerationEvent::Failed { .. }
            )
        })
        .count()
}

#[test]
fn arbitrary_splits_keep_thinking_and_content_separate() -> Result<(), Box<dyn Error>> {
    let input = concat!(
        "{\"message\":{\"role\":\"assistant\",\"thinking\":\"Think 🧠\",\"content\":\"\"},\"done\":false}\n",
        "{\"message\":{\"role\":\"assistant\",\"thinking\":\"\",\"content\":\"Hello 🌍\"},\"done\":false}\n",
        "{\"message\":{\"role\":\"assistant\",\"content\":\"!\"},\"done\":true,\"done_reason\":\"stop\"}\n"
    );
    let mut decoder = QwenStreamDecoder::new("qwen-split")?;
    let events = by_byte(&mut decoder, input);
    let reasoning = events
        .iter()
        .filter_map(|event| match event {
            GenerationEvent::ReasoningDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    let content = events
        .iter()
        .filter_map(|event| match event {
            GenerationEvent::FinalContentDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    assert_eq!(reasoning, "Think 🧠");
    assert_eq!(content, "Hello 🌍!");
    let preview = events
        .iter()
        .filter_map(|event| match event {
            GenerationEvent::ContentPreviewDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    assert_eq!(preview, content);
    assert_eq!(
        events.last(),
        Some(&GenerationEvent::Completed {
            reason: GenerationCompletion::Stop
        })
    );
    assert_eq!(terminal_count(&events), 1);
    assert!(decoder.push(b"ignored").is_empty());
    assert!(decoder.finish(GenerationCompletion::Stop).is_empty());
    Ok(())
}

#[test]
fn provider_thinking_delimiters_never_escape_the_reasoning_channel() -> Result<(), Box<dyn Error>> {
    let input = concat!(
        "{\"message\":{\"role\":\"assistant\",\"thinking\":\"<thi\"},\"done\":false}\n",
        "{\"message\":{\"role\":\"assistant\",\"thinking\":\"nk>Consider the tool.</thi\"},\"done\":false}\n",
        "{\"message\":{\"role\":\"assistant\",\"thinking\":\"nk>\",\"content\":\"</thi\"},\"done\":false}\n",
        "{\"message\":{\"role\":\"assistant\",\"content\":\"nk>Final answer\"},\"done\":false}\n",
        "{\"done\":true,\"done_reason\":\"stop\"}\n"
    );
    let mut decoder = QwenStreamDecoder::new("qwen-delimiters")?;
    let events = by_byte(&mut decoder, input);
    let reasoning = events
        .iter()
        .filter_map(|event| match event {
            GenerationEvent::ReasoningDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    let content = events
        .iter()
        .filter_map(|event| match event {
            GenerationEvent::FinalContentDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();

    assert_eq!(reasoning, "Consider the tool.");
    assert_eq!(content, "Final answer");
    assert!(!reasoning.contains("think>"));
    assert!(!content.contains("think>"));
    Ok(())
}

#[test]
fn tool_bearing_content_keeps_its_native_channel() -> Result<(), Box<dyn Error>> {
    let input = concat!(
        "{\"message\":{\"role\":\"assistant\",\"content\":\"I will call it.</thi\"},\"done\":false}\n",
        "{\"message\":{\"role\":\"assistant\",\"content\":\"nk>\",\"tool_calls\":[{\"function\":{\"name\":\"files.read\",\"arguments\":{\"path\":\"a\"}}}]},\"done\":false}\n",
        "{\"done\":true,\"done_reason\":\"stop\"}\n"
    );
    let mut decoder = QwenStreamDecoder::new("tool-content-delimiter")?;
    let events = by_byte(&mut decoder, input);
    let content = events
        .iter()
        .filter_map(|event| match event {
            GenerationEvent::FinalContentDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();

    assert_eq!(content, "I will call it.");
    assert!(!content.contains("think>"));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, GenerationEvent::ToolCall { .. }))
    );
    Ok(())
}

#[test]
fn terminal_provider_metrics_are_emitted_without_inventing_missing_memory()
-> Result<(), Box<dyn Error>> {
    let mut decoder = QwenStreamDecoder::new("qwen-metrics")?;
    let events = decoder.push(
        b"{\"done\":true,\"done_reason\":\"stop\",\"prompt_eval_count\":120,\"eval_count\":30,\"prompt_eval_duration\":2000000000,\"eval_duration\":3000000000,\"total_duration\":5500000000}\n",
    );
    assert_eq!(
        events.first(),
        Some(&GenerationEvent::Metrics {
            metrics: crate::GenerationMetrics {
                prompt_tokens: Some(120),
                cached_prompt_tokens: None,
                cache_write_prompt_tokens: None,
                generated_tokens: Some(30),
                prompt_evaluation_ns: Some(2_000_000_000),
                generation_ns: Some(3_000_000_000),
                total_ns: Some(5_500_000_000),
                load_ns: None,
                peak_memory_bytes: None,
            }
        })
    );
    assert!(matches!(
        events.last(),
        Some(GenerationEvent::Completed { .. })
    ));
    Ok(())
}

#[test]
fn tool_calls_are_buffered_and_missing_ids_are_deterministic() -> Result<(), Box<dyn Error>> {
    let first = json!({
        "message": {
            "role": "assistant",
            "thinking": "Need two lookups",
            "content": "",
            "tool_calls": [{
                "function": {"index": 0, "name": "files.read", "arguments": {"path": "a.txt"}}
            }]
        },
        "done": false
    });
    let second = json!({
        "message": {
            "role": "assistant",
            "content": "",
            "tool_calls": [{
                "id": "provider-call-b",
                "function": {"index": 1, "name": "files.stat", "arguments": {"path": "b.txt"}}
            }]
        },
        "done": false
    });
    let done = json!({"done": true, "done_reason": "stop"});
    let mut decoder = QwenStreamDecoder::new("request-tools")?;

    let early = decoder.push(format!("{first}\n{second}\n").as_bytes());
    assert!(
        early
            .iter()
            .all(|event| !matches!(event, GenerationEvent::ToolCall { .. }))
    );
    let terminal = decoder.push(format!("{done}\n").as_bytes());

    assert_eq!(
        terminal,
        vec![
            GenerationEvent::ToolCall {
                call: ParsedToolCall {
                    call_id: "request-tools:qwen-tool:1".to_owned(),
                    recipient: "files.read".to_owned(),
                    name: "files.read".to_owned(),
                    arguments: BTreeMap::from([("path".to_owned(), json!("a.txt"))]),
                }
            },
            GenerationEvent::ToolCall {
                call: ParsedToolCall {
                    call_id: "provider-call-b".to_owned(),
                    recipient: "files.stat".to_owned(),
                    name: "files.stat".to_owned(),
                    arguments: BTreeMap::from([("path".to_owned(), json!("b.txt"))]),
                }
            },
            GenerationEvent::Completed {
                reason: GenerationCompletion::Stop
            }
        ]
    );
    assert_eq!(terminal_count(&terminal), 1);
    Ok(())
}

#[test]
fn mixed_content_is_preserved_before_releasing_tool_calls() -> Result<(), Box<dyn Error>> {
    let input = concat!(
        "{\"message\":{\"role\":\"assistant\",\"content\":\"I will also call a tool\"},\"done\":false}\n",
        "{\"message\":{\"role\":\"assistant\",\"tool_calls\":[{\"function\":{\"name\":\"files.read\",\"arguments\":{\"path\":\"a\"}}}]},\"done\":false}\n",
        "{\"done\":true,\"done_reason\":\"stop\"}\n"
    );
    let mut decoder = QwenStreamDecoder::new("mixed")?;
    let events = decoder.push(input.as_bytes());
    assert!(
        events
            .iter()
            .all(|event| !matches!(event, GenerationEvent::ReasoningDelta { .. }))
    );
    assert_eq!(
        events,
        vec![
            GenerationEvent::ContentPreviewDelta {
                text: "I will also call a tool".to_owned()
            },
            GenerationEvent::FinalContentDelta {
                text: "I will also call a tool".to_owned()
            },
            GenerationEvent::ToolCall {
                call: ParsedToolCall {
                    call_id: "mixed:qwen-tool:1".to_owned(),
                    recipient: "files.read".to_owned(),
                    name: "files.read".to_owned(),
                    arguments: BTreeMap::from([("path".to_owned(), json!("a"))]),
                }
            },
            GenerationEvent::Completed {
                reason: GenerationCompletion::Stop
            }
        ]
    );
    assert_eq!(terminal_count(&events), 1);
    Ok(())
}

#[test]
fn answer_preview_streams_before_authoritative_tool_free_completion() -> Result<(), Box<dyn Error>>
{
    let mut decoder = QwenStreamDecoder::new("buffered-content")?;
    let early = decoder.push(
        b"{\"message\":{\"role\":\"assistant\",\"content\":\"final answer\"},\"done\":false}\n",
    );
    assert_eq!(
        early,
        vec![GenerationEvent::ContentPreviewDelta {
            text: "final answer".to_owned()
        }]
    );
    assert_eq!(
        decoder.push(b"{\"done\":true,\"done_reason\":\"stop\"}\n"),
        vec![
            GenerationEvent::FinalContentDelta {
                text: "final answer".to_owned()
            },
            GenerationEvent::Completed {
                reason: GenerationCompletion::Stop
            }
        ]
    );
    Ok(())
}

#[test]
fn repeated_and_malformed_calls_fail_once() -> Result<(), Box<dyn Error>> {
    let call = "{\"function\":{\"name\":\"files.read\",\"arguments\":{\"path\":\"a\"}}}";
    let repeated = format!(
        "{{\"message\":{{\"role\":\"assistant\",\"tool_calls\":[{call}]}},\"done\":false}}\n{{\"message\":{{\"role\":\"assistant\",\"tool_calls\":[{call}]}},\"done\":false}}\n"
    );
    let mut decoder = QwenStreamDecoder::new("repeat")?;
    let events = decoder.push(repeated.as_bytes());
    assert_eq!(
        events,
        vec![GenerationEvent::Failed {
            failure: ModelOutputFailureKind::MalformedToolCall
        }]
    );
    assert!(decoder.cancel().is_empty());

    let mut malformed = QwenStreamDecoder::new("malformed")?;
    let events = malformed.push(
        b"{\"message\":{\"role\":\"assistant\",\"tool_calls\":[{\"function\":{\"name\":\"bad name\",\"arguments\":[]}}]},\"done\":true}\n",
    );
    assert_eq!(terminal_count(&events), 1);
    assert!(matches!(
        events.last(),
        Some(GenerationEvent::Failed {
            failure: ModelOutputFailureKind::MalformedToolCall
        })
    ));
    Ok(())
}

#[test]
fn truncated_json_disconnect_and_cancellation_are_distinct_terminals() -> Result<(), Box<dyn Error>>
{
    let mut truncated = QwenStreamDecoder::new("truncated")?;
    assert!(truncated.push(b"{\"message\":").is_empty());
    let events = truncated.finish(GenerationCompletion::Stop);
    assert_eq!(terminal_count(&events), 1);
    assert!(matches!(
        events.last(),
        Some(GenerationEvent::Failed {
            failure: ModelOutputFailureKind::InvalidProtocol
        })
    ));

    let mut disconnected = QwenStreamDecoder::new("disconnect")?;
    let events = disconnected.fail_upstream();
    assert_eq!(
        events,
        vec![GenerationEvent::Failed {
            failure: ModelOutputFailureKind::UpstreamFailure
        }]
    );

    let mut cancelled = QwenStreamDecoder::new("cancel")?;
    let mut events = cancelled
        .push(b"{\"message\":{\"role\":\"assistant\",\"thinking\":\"working\"},\"done\":false}\n");
    events.extend(cancelled.cancel());
    assert!(matches!(
        events.first(),
        Some(GenerationEvent::ReasoningDelta { .. })
    ));
    assert_eq!(events.last(), Some(&GenerationEvent::Cancelled));
    assert_eq!(terminal_count(&events), 1);
    Ok(())
}

#[test]
fn empty_final_content_and_length_are_valid_native_completions() -> Result<(), Box<dyn Error>> {
    let mut empty = QwenStreamDecoder::new("empty")?;
    assert_eq!(
        empty.push(b"{\"message\":{\"role\":\"assistant\",\"content\":\"\"},\"done\":true,\"done_reason\":\"stop\"}\n"),
        vec![GenerationEvent::Completed {
            reason: GenerationCompletion::Stop
        }]
    );

    let mut length = QwenStreamDecoder::new("length")?;
    assert_eq!(
        length.push(b"{\"message\":{\"role\":\"assistant\",\"content\":\"partial\"},\"done\":true,\"done_reason\":\"length\"}\n").last(),
        Some(&GenerationEvent::Completed {
            reason: GenerationCompletion::Length
        })
    );
    Ok(())
}

#[test]
fn invalid_scope_utf8_and_server_error_are_content_free() -> Result<(), Box<dyn Error>> {
    assert!(QwenStreamDecoder::new("bad scope").is_err());

    let mut utf8 = QwenStreamDecoder::new("utf8")?;
    assert_eq!(
        utf8.push(&[0xff, b'\n']),
        vec![GenerationEvent::Failed {
            failure: ModelOutputFailureKind::InvalidEncoding
        }]
    );

    let mut server = QwenStreamDecoder::new("server")?;
    let events = server.push(b"{\"error\":\"secret raw server output\"}\n");
    assert_eq!(
        events,
        vec![GenerationEvent::Failed {
            failure: ModelOutputFailureKind::UpstreamFailure
        }]
    );
    Ok(())
}

#[test]
fn preview_is_cleared_on_cancel_disconnect_and_invalid_late_tool() -> Result<(), Box<dyn Error>> {
    for ending in ["cancel", "disconnect", "eof", "malformed_tool"] {
        let mut decoder = QwenStreamDecoder::new(ending)?;
        let early =
            decoder.push(b"{\"message\":{\"role\":\"assistant\",\"content\":\"Uncommitted draft\"},\"done\":false}\n");
        assert!(matches!(
            early.as_slice(),
            [GenerationEvent::ContentPreviewDelta { .. }]
        ));
        let terminal = match ending {
            "cancel" => decoder.cancel(),
            "disconnect" => decoder.fail_upstream(),
            "eof" => decoder.finish(GenerationCompletion::Stop),
            _ => decoder.push(b"{\"message\":{\"role\":\"assistant\",\"tool_calls\":[{\"function\":{\"name\":\"bad name\",\"arguments\":[]}}]},\"done\":true}\n"),
        };
        assert_eq!(
            terminal.first(),
            Some(&GenerationEvent::ContentPreviewReset),
            "{ending}"
        );
        assert_eq!(terminal_count(&terminal), 1, "{ending}");
        let expected = match ending {
            "cancel" => GenerationEvent::Cancelled,
            "disconnect" => GenerationEvent::Failed {
                failure: ModelOutputFailureKind::UpstreamFailure,
            },
            "eof" => GenerationEvent::Failed {
                failure: ModelOutputFailureKind::UnexpectedEnd,
            },
            _ => GenerationEvent::Failed {
                failure: ModelOutputFailureKind::MalformedToolCall,
            },
        };
        assert_eq!(terminal.last(), Some(&expected), "{ending}");
        assert!(
            terminal.iter().all(|event| !matches!(
                event,
                GenerationEvent::FinalContentDelta { .. } | GenerationEvent::ToolCall { .. }
            )),
            "{ending}"
        );
        assert!(decoder.cancel().is_empty());
    }
    Ok(())
}

#[test]
fn large_unicode_preview_uses_bounded_utf8_deltas() -> Result<(), Box<dyn Error>> {
    let text = "🌍".repeat(20_000);
    let frame = json!({"message": {"role": "assistant", "content": text}, "done": false});
    let mut decoder = QwenStreamDecoder::new("large-preview")?;
    let events = decoder.push(format!("{frame}\n").as_bytes());
    let mut preview = String::new();
    for event in events {
        let GenerationEvent::ContentPreviewDelta { text } = event else {
            return Err("unexpected authoritative event before completion".into());
        };
        assert!(text.len() <= 32 * 1024);
        preview.push_str(&text);
    }
    assert_eq!(preview, text);
    Ok(())
}
