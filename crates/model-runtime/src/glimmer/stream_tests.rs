//! Chunk-boundary and terminal-invariant tests for streamed Glimmer output.

use std::{collections::BTreeMap, error::Error};

use serde_json::json;

use super::{GlimmerStreamParser, stream::GlimmerStreamError};
use crate::{
    GenerationCompletion as StreamCompletion, GenerationEvent as GlimmerStreamEvent,
    ModelOutputFailureKind, ParsedToolCall,
};

fn push_by_byte(parser: &mut GlimmerStreamParser, input: &str) -> Vec<GlimmerStreamEvent> {
    input
        .as_bytes()
        .iter()
        .flat_map(|byte| parser.push(&[*byte]))
        .collect()
}

fn terminal_count(events: &[GlimmerStreamEvent]) -> usize {
    events
        .iter()
        .filter(|event| {
            matches!(
                event,
                GlimmerStreamEvent::Completed { .. }
                    | GlimmerStreamEvent::Cancelled
                    | GlimmerStreamEvent::Failed { .. }
            )
        })
        .count()
}

#[test]
fn arbitrary_chunks_and_incomplete_utf8_preserve_reasoning_and_content()
-> Result<(), Box<dyn Error>> {
    let mut parser = GlimmerStreamParser::new("request-1")?;
    let input = concat!(
        " to=self<|message|>Think 🧠 carefully<|eom|>",
        "<|start|>assistant to=user<|message|>Hello 🌍<|eot|>"
    );
    let mut events = push_by_byte(&mut parser, input);
    events.extend(parser.finish(StreamCompletion::Stop));

    let reasoning = events
        .iter()
        .filter_map(|event| match event {
            GlimmerStreamEvent::ReasoningDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    let content = events
        .iter()
        .filter_map(|event| match event {
            GlimmerStreamEvent::FinalContentDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    assert_eq!(reasoning, "Think 🧠 carefully");
    assert_eq!(content, "Hello 🌍");
    assert_eq!(
        events.last(),
        Some(&GlimmerStreamEvent::Completed {
            reason: StreamCompletion::Stop
        })
    );
    assert_eq!(terminal_count(&events), 1);
    assert!(parser.finish(StreamCompletion::Stop).is_empty());
    assert!(parser.push(b"ignored").is_empty());
    Ok(())
}

#[test]
fn full_anchor_is_accepted_when_the_backend_echoes_it() -> Result<(), Box<dyn Error>> {
    let mut parser = GlimmerStreamParser::new("request-anchor")?;
    let mut events = parser.push(b"<|start|>assistant to=user<|message|>Answer<|eot|>");
    events.extend(parser.finish(StreamCompletion::Stop));
    assert_eq!(
        events,
        vec![
            GlimmerStreamEvent::FinalContentDelta {
                text: "Answer".to_owned()
            },
            GlimmerStreamEvent::Completed {
                reason: StreamCompletion::Stop
            }
        ]
    );
    Ok(())
}

#[test]
fn a_large_transport_chunk_is_streamed_as_bounded_deltas() -> Result<(), Box<dyn Error>> {
    let mut parser = GlimmerStreamParser::new("request-large-chunk")?;
    let answer = "a".repeat(600 * 1024);
    let input = format!(" to=user<|message|>{answer}<|eot|>");
    let mut events = parser.push(input.as_bytes());
    events.extend(parser.finish(StreamCompletion::Stop));

    let deltas = events
        .iter()
        .filter_map(|event| match event {
            GlimmerStreamEvent::FinalContentDelta { text } => Some(text),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        deltas.iter().map(|text| text.len()).sum::<usize>(),
        answer.len()
    );
    assert!(deltas.iter().all(|text| text.len() <= 32 * 1024));
    assert_eq!(terminal_count(&events), 1);
    assert!(matches!(
        events.last(),
        Some(GlimmerStreamEvent::Completed {
            reason: StreamCompletion::Stop
        })
    ));
    Ok(())
}

#[test]
fn interleaved_reasoning_tool_and_content_keep_event_order_and_call_id()
-> Result<(), Box<dyn Error>> {
    let mut parser = GlimmerStreamParser::new("request-tools")?;
    let input = concat!(
        " to=self<|message|>Need lookup<|eom|>",
        "<|start|>assistant to=zeta.lookup<|message|>",
        "<atem:function_calls>\n",
        "<atem:invoke name=\"zeta.lookup\">\n",
        "<atem:parameter name=\"limit\">3</atem:parameter>\n",
        "<atem:parameter name=\"options\">{\"fresh\": true}</atem:parameter>\n",
        "<atem:parameter name=\"query\">rust docs</atem:parameter>\n",
        "</atem:invoke>\n</atem:function_calls><|eom|>",
        "<|start|>assistant to=user<|message|>Found it<|eot|>"
    );
    let mut events = input
        .as_bytes()
        .chunks(3)
        .flat_map(|chunk| parser.push(chunk))
        .collect::<Vec<_>>();
    events.extend(parser.finish(StreamCompletion::Stop));

    let tool_index = events
        .iter()
        .position(|event| matches!(event, GlimmerStreamEvent::ToolCall { .. }))
        .ok_or("tool event missing")?;
    let first_content = events
        .iter()
        .position(|event| matches!(event, GlimmerStreamEvent::FinalContentDelta { .. }))
        .ok_or("content event missing")?;
    assert!(
        events[..tool_index]
            .iter()
            .any(|event| matches!(event, GlimmerStreamEvent::ReasoningDelta { .. }))
    );
    assert!(tool_index < first_content);
    assert_eq!(
        events.get(tool_index),
        Some(&GlimmerStreamEvent::ToolCall {
            call: ParsedToolCall {
                call_id: "request-tools:tool:1".to_owned(),
                recipient: "zeta.lookup".to_owned(),
                name: "zeta.lookup".to_owned(),
                arguments: BTreeMap::from([
                    ("limit".to_owned(), json!(3)),
                    ("options".to_owned(), json!({"fresh": true})),
                    ("query".to_owned(), json!("rust docs")),
                ]),
            }
        })
    );
    assert_eq!(terminal_count(&events), 1);
    Ok(())
}

#[test]
fn length_limit_completes_only_partial_content() -> Result<(), Box<dyn Error>> {
    let mut content_parser = GlimmerStreamParser::new("request-length")?;
    let mut events = content_parser.push(" to=user<|message|>Partial 🌍".as_bytes());
    events.extend(content_parser.finish(StreamCompletion::Length));
    let content = events
        .iter()
        .filter_map(|event| match event {
            GlimmerStreamEvent::FinalContentDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    assert_eq!(content, "Partial 🌍");
    assert_eq!(
        events.last(),
        Some(&GlimmerStreamEvent::Completed {
            reason: StreamCompletion::Length
        })
    );

    let mut reasoning_parser = GlimmerStreamParser::new("request-truncated-reasoning")?;
    let mut failed = reasoning_parser.push(b" to=self<|message|>unfinished");
    failed.extend(reasoning_parser.finish(StreamCompletion::Length));
    assert!(matches!(
        failed.last(),
        Some(GlimmerStreamEvent::Failed {
            failure: ModelOutputFailureKind::UnexpectedEnd
        })
    ));
    Ok(())
}

#[test]
fn malformed_utf8_protocol_and_atem_fail_once() -> Result<(), Box<dyn Error>> {
    let mut utf8 = GlimmerStreamParser::new("request-utf8")?;
    let events = utf8.push(&[0xff]);
    assert_eq!(
        events,
        vec![GlimmerStreamEvent::Failed {
            failure: ModelOutputFailureKind::InvalidEncoding
        }]
    );
    assert!(utf8.cancel().is_empty());

    let mut truncated_utf8 = GlimmerStreamParser::new("request-truncated-utf8")?;
    assert!(truncated_utf8.push(&[0xf0, 0x9f]).is_empty());
    assert_eq!(
        truncated_utf8.finish(StreamCompletion::Stop),
        vec![GlimmerStreamEvent::Failed {
            failure: ModelOutputFailureKind::InvalidEncoding
        }]
    );

    let mut protocol = GlimmerStreamParser::new("request-protocol")?;
    let events = protocol.push(b" to=user<|message|>bad<|start|>assistant");
    assert!(matches!(
        events.last(),
        Some(GlimmerStreamEvent::Failed {
            failure: ModelOutputFailureKind::InvalidProtocol
        })
    ));
    assert_eq!(terminal_count(&events), 1);

    let mut atem = GlimmerStreamParser::new("request-atem")?;
    let events = atem.push(
        concat!(
            " to=zeta.lookup<|message|>",
            "<atem:function_calls><atem:invoke name=\"zeta.other\">",
            "</atem:invoke></atem:function_calls><|eot|>"
        )
        .as_bytes(),
    );
    assert!(matches!(
        events.last(),
        Some(GlimmerStreamEvent::Failed {
            failure: ModelOutputFailureKind::RecipientMismatch
        })
    ));
    assert_eq!(terminal_count(&events), 1);
    Ok(())
}

#[test]
fn duplicate_parameters_and_incomplete_tool_calls_fail_safely() -> Result<(), Box<dyn Error>> {
    let mut duplicate = GlimmerStreamParser::new("request-duplicate")?;
    let events = duplicate.push(
        concat!(
            " to=zeta.lookup<|message|><atem:function_calls>\n",
            "<atem:invoke name=\"zeta.lookup\">\n",
            "<atem:parameter name=\"query\">one</atem:parameter>\n",
            "<atem:parameter name=\"query\">two</atem:parameter>\n",
            "</atem:invoke>\n</atem:function_calls><|eot|>"
        )
        .as_bytes(),
    );
    assert!(matches!(
        events.last(),
        Some(GlimmerStreamEvent::Failed {
            failure: ModelOutputFailureKind::DuplicateToolParameter
        })
    ));

    let mut incomplete = GlimmerStreamParser::new("request-incomplete")?;
    let mut events = incomplete.push(
        concat!(
            " to=zeta.lookup<|message|><atem:function_calls>\n",
            "<atem:invoke name=\"zeta.lookup\">"
        )
        .as_bytes(),
    );
    events.extend(incomplete.finish(StreamCompletion::Stop));
    assert!(matches!(
        events.last(),
        Some(GlimmerStreamEvent::Failed {
            failure: ModelOutputFailureKind::UnexpectedEnd
        })
    ));
    Ok(())
}

#[test]
fn cancellation_and_upstream_failure_are_single_terminal_events() -> Result<(), Box<dyn Error>> {
    let mut cancelled = GlimmerStreamParser::new("request-cancel")?;
    let mut events = cancelled.push(b" to=user<|message|>partial");
    events.extend(cancelled.cancel());
    assert_eq!(terminal_count(&events), 1);
    assert!(matches!(events.last(), Some(GlimmerStreamEvent::Cancelled)));
    assert!(cancelled.cancel().is_empty());
    assert!(cancelled.finish(StreamCompletion::Stop).is_empty());

    let mut upstream = GlimmerStreamParser::new("request-upstream")?;
    assert_eq!(
        upstream.fail_upstream(),
        vec![GlimmerStreamEvent::Failed {
            failure: ModelOutputFailureKind::UpstreamFailure
        }]
    );
    assert!(upstream.fail_upstream().is_empty());

    let mut pressured = GlimmerStreamParser::new("request-pressure")?;
    assert_eq!(
        pressured.fail_memory_pressure(),
        vec![GlimmerStreamEvent::Failed {
            failure: ModelOutputFailureKind::MemoryPressure
        }]
    );
    assert!(pressured.fail_memory_pressure().is_empty());
    Ok(())
}

#[test]
fn invalid_generation_ids_are_rejected_before_streaming() {
    assert!(matches!(
        GlimmerStreamParser::new("bad id"),
        Err(GlimmerStreamError::InvalidGenerationId)
    ));
}
