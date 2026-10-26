//! Portable speech model installation and verified-use boundary.

pub(crate) mod artifact;
mod helper;
mod protocol;
mod recognition;
mod signal;
mod store;

pub use helper::{WhisperHelper, WhisperHelperConfig};
pub use recognition::{
    AudioEncoding, AudioFormat, AudioFrame, RecognitionAcceleration, RecognitionCancellation,
    RecognitionCapabilities, RecognitionConfig, RecognitionError, RecognitionEvent,
    RecognitionSession, SpeechRecognizer,
};
pub use store::{SpeechModelError, SpeechModelLease, SpeechModelStatus, SpeechModelStore};

/// Native source revision qualified by ASR-001; no platform acceleration implied.
pub const WHISPER_RUNTIME_REVISION: &str = "371b5a7561823ab2bb32142d2751e35e7534727b";
