//! Display-only projection of model text; never supplies persisted answer content.

use ipc_types::{GenerationEvent, RequestId};
use model_runtime::GenerationEvent as ModelGenerationEvent;

use super::next_sequence;

pub(super) struct RoundStreamDisplay {
    request_id: RequestId,
    round: u32,
    previewed: bool,
}

impl RoundStreamDisplay {
    pub(super) fn new(request_id: RequestId, round: usize) -> Self {
        Self {
            request_id,
            round: u32::try_from(round).unwrap_or(u32::MAX),
            previewed: false,
        }
    }

    pub(super) fn reset(&mut self, sequence: &mut u32) -> GenerationEvent {
        self.previewed = false;
        GenerationEvent::TextPreviewReset {
            request_id: self.request_id.clone(),
            sequence: next_sequence(sequence),
            round: self.round,
        }
    }

    pub(super) fn project(
        &mut self,
        event: &ModelGenerationEvent,
        sequence: &mut u32,
    ) -> Option<GenerationEvent> {
        match event {
            ModelGenerationEvent::ContentPreviewDelta { text } => {
                self.previewed = true;
                Some(GenerationEvent::TextPreviewDelta {
                    request_id: self.request_id.clone(),
                    sequence: next_sequence(sequence),
                    round: self.round,
                    text: text.clone(),
                })
            }
            ModelGenerationEvent::ContentPreviewReset => Some(self.reset(sequence)),
            // Qwen has already displayed this text. The request's authoritative
            // Final event commits it once; Glimmer still streams directly here.
            ModelGenerationEvent::FinalContentDelta { text } if !self.previewed => {
                Some(GenerationEvent::TextDelta {
                    request_id: self.request_id.clone(),
                    sequence: next_sequence(sequence),
                    text: text.clone(),
                })
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_is_forwarded_but_authoritative_echo_is_not_duplicated() {
        let mut display = RoundStreamDisplay::new(RequestId("request".to_owned()), 2);
        let mut sequence = 10;
        assert!(matches!(
            display.reset(&mut sequence),
            GenerationEvent::TextPreviewReset {
                sequence: 10,
                round: 2,
                ..
            }
        ));
        assert!(
            matches!(display.project(&ModelGenerationEvent::ContentPreviewDelta {
            text: "answer".to_owned(),
        }, &mut sequence), Some(GenerationEvent::TextPreviewDelta {
            sequence: 11, round: 2, text, ..
        }) if text == "answer")
        );
        assert!(
            display
                .project(
                    &ModelGenerationEvent::FinalContentDelta {
                        text: "answer".to_owned(),
                    },
                    &mut sequence
                )
                .is_none()
        );
        assert_eq!(sequence, 12);
    }

    #[test]
    fn tool_reclassification_and_new_rounds_clear_the_display() {
        let mut display = RoundStreamDisplay::new(RequestId("request".to_owned()), 0);
        let mut sequence = 0;
        let _ = display.project(
            &ModelGenerationEvent::ContentPreviewDelta {
                text: "preamble".to_owned(),
            },
            &mut sequence,
        );
        assert!(matches!(
            display.project(&ModelGenerationEvent::ContentPreviewReset, &mut sequence),
            Some(GenerationEvent::TextPreviewReset { round: 0, .. })
        ));
        assert!(
            display
                .project(
                    &ModelGenerationEvent::ReasoningDelta {
                        text: "preamble".to_owned(),
                    },
                    &mut sequence
                )
                .is_none()
        );
        let mut next = RoundStreamDisplay::new(RequestId("request".to_owned()), 1);
        assert!(matches!(
            next.reset(&mut sequence),
            GenerationEvent::TextPreviewReset { round: 1, .. }
        ));
    }

    #[test]
    fn non_preview_adapters_keep_direct_content_streaming() {
        let mut display = RoundStreamDisplay::new(RequestId("request".to_owned()), 0);
        let mut sequence = 0;
        for text in ["Hello", " world"] {
            assert!(
                matches!(display.project(&ModelGenerationEvent::FinalContentDelta {
                text: text.to_owned(),
            }, &mut sequence), Some(GenerationEvent::TextDelta { text: actual, .. }) if actual == text)
            );
        }
        assert_eq!(sequence, 2);
    }
}
