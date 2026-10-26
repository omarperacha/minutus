//! Publisher sampling profiles for the native Qwen3.8 chat boundary.

use serde::{Serialize, Serializer, ser::SerializeStruct};

/// Fixed profiles keep floating-point configuration out of runner identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum QwenSampling {
    Thinking,
    Instruct,
}

impl QwenSampling {
    pub(super) const fn for_thinking(enabled: bool) -> Self {
        if enabled {
            Self::Thinking
        } else {
            Self::Instruct
        }
    }
}

impl Serialize for QwenSampling {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let (temperature, top_p, presence_penalty) = match self {
            Self::Thinking => (1.0, 0.95, 0.0),
            Self::Instruct => (0.7, 0.8, 1.5),
        };
        let mut state = serializer.serialize_struct("QwenSampling", 6)?;
        state.serialize_field("temperature", &temperature)?;
        state.serialize_field("top_p", &top_p)?;
        state.serialize_field("top_k", &20)?;
        state.serialize_field("min_p", &0.0)?;
        state.serialize_field("presence_penalty", &presence_penalty)?;
        state.serialize_field("repeat_penalty", &1.0)?;
        state.end()
    }
}
