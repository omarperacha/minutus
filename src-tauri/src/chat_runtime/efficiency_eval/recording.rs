//! Synthetic-only event aggregation, separate from production diagnostics.

use std::time::Instant;

use ipc_types::{ChatGenerationEvent, GenerationEvent};
use serde_json::{Value, json};

pub(super) struct Recording {
    started: Instant,
    first_preview_ms: Option<u128>,
    first_content_ms: Option<u128>,
    first_reasoning_ms: Option<u128>,
    first_activity_ms: Option<u128>,
    terminal: Option<Value>,
}

impl Recording {
    pub(super) fn new() -> Self {
        Self {
            started: Instant::now(),
            first_preview_ms: None,
            first_content_ms: None,
            first_reasoning_ms: None,
            first_activity_ms: None,
            terminal: None,
        }
    }

    pub(super) fn observe(&mut self, payload: &str) {
        let Ok(event) = serde_json::from_str::<ChatGenerationEvent>(payload) else {
            return;
        };
        let elapsed = self.started.elapsed().as_millis();
        match event.event {
            GenerationEvent::TextPreviewDelta { text, .. } if !text.is_empty() => {
                self.first_preview_ms.get_or_insert(elapsed);
            }
            GenerationEvent::TextDelta { text, .. } if !text.is_empty() => {
                self.first_content_ms.get_or_insert(elapsed);
            }
            GenerationEvent::ReasoningDelta { text, .. } if !text.is_empty() => {
                self.first_reasoning_ms.get_or_insert(elapsed);
            }
            GenerationEvent::Activity { .. } => {
                self.first_activity_ms.get_or_insert(elapsed);
            }
            event @ (GenerationEvent::Final { .. }
            | GenerationEvent::Error { .. }
            | GenerationEvent::Cancelled { .. }) => {
                // The desktop may commit buffered content only in its Final
                // event, without emitting an earlier authoritative TextDelta.
                if let GenerationEvent::Final { text, .. } = &event
                    && !text.is_empty()
                {
                    self.first_content_ms.get_or_insert(elapsed);
                }
                self.terminal = serde_json::to_value(event).ok();
            }
            _ => {}
        }
    }

    pub(super) fn finish(&self) -> Value {
        json!({
            "wall_ms": self.started.elapsed().as_millis(),
            "first_preview_ms": self.first_preview_ms,
            "first_content_ms": self.first_content_ms,
            "first_reasoning_ms": self.first_reasoning_ms,
            "first_activity_ms": self.first_activity_ms,
            "terminal": self.terminal,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasoning_and_preview_do_not_count_as_final_content() {
        let mut recording = Recording::new();
        recording.observe(r#"{"conversationId":"c","event":{"type":"reasoningDelta","requestId":"r","sequence":0,"text":"private"}}"#);
        recording.observe(r#"{"conversationId":"c","event":{"type":"textPreviewDelta","requestId":"r","sequence":1,"round":0,"text":"preview"}}"#);
        let result = recording.finish();
        assert!(result["first_reasoning_ms"].is_number());
        assert!(result["first_preview_ms"].is_number());
        assert!(result["first_content_ms"].is_null());
        assert!(result["terminal"].is_null());
        recording.observe(r#"{"conversationId":"c","event":{"type":"final","requestId":"r","sequence":2,"messageId":"m","text":"answer","finishReason":"stop"}}"#);
        let result = recording.finish();
        assert!(result["first_content_ms"].is_number());
        assert_eq!(result["terminal"]["type"], "final");
    }
}
