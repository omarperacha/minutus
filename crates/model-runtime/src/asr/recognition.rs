use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use super::SpeechModelLease;

/// Fixed encoding carried by normalized audio frames.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioEncoding {
    /// Native-endian IEEE 754 samples in `[-1, 1]`.
    Float32,
}

/// Explicit audio shape at the recognizer boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioFormat {
    /// Samples per second.
    pub sample_rate_hz: u32,
    /// Interleaved channel count.
    pub channels: u8,
    /// Sample encoding.
    pub encoding: AudioEncoding,
}

impl AudioFormat {
    /// Whisper's normalized portable input format.
    pub const WHISPER_PCM: Self = Self {
        sample_rate_hz: 16_000,
        channels: 1,
        encoding: AudioEncoding::Float32,
    };
}

/// One ordered, bounded block of normalized audio.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioFrame {
    /// Zero-based sequence within one session.
    pub sequence: u64,
    /// Format repeated on every frame to reject accidental changes.
    pub format: AudioFormat,
    /// Interleaved samples.
    pub samples: Vec<f32>,
}

/// Hardware-neutral acceleration request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecognitionAcceleration {
    /// Use an available backend, retaining CPU compatibility.
    Automatic,
    /// Require the portable CPU path.
    Cpu,
}

/// Validated options for one recognition session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecognitionConfig {
    language: Option<String>,
    acceleration: RecognitionAcceleration,
}

impl RecognitionConfig {
    /// Creates automatic-language recognition with automatic acceleration.
    #[must_use]
    pub const fn automatic() -> Self {
        Self {
            language: None,
            acceleration: RecognitionAcceleration::Automatic,
        }
    }
    /// Selects a supported ISO-style language code; the helper performs the final support check.
    ///
    /// # Errors
    /// Rejects empty, unbounded or non-canonical codes.
    pub fn with_language(mut self, language: impl Into<String>) -> Result<Self, RecognitionError> {
        let language = language.into();
        if !(2..=8).contains(&language.len())
            || !language
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b == b'-')
        {
            return Err(RecognitionError::InvalidConfiguration);
        }
        self.language = Some(language);
        Ok(self)
    }
    /// Selects a hardware-neutral execution preference.
    #[must_use]
    pub const fn with_acceleration(mut self, acceleration: RecognitionAcceleration) -> Self {
        self.acceleration = acceleration;
        self
    }
    pub(super) fn language(&self) -> Option<&str> {
        self.language.as_deref()
    }
    pub(super) const fn acceleration(&self) -> RecognitionAcceleration {
        self.acceleration
    }
}

/// Streaming claims exposed independently of a concrete model/backend name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecognitionCapabilities {
    /// Audio may arrive in frames; text is emitted only after finish.
    FinalOnly,
    /// Future recognizers may replace provisional segments by revision.
    RevisablePartials,
}

/// Ordered terminal recognition output. No event submits a chat message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecognitionEvent {
    /// Valid editable transcript in the detected/requested spoken language.
    Final {
        /// Monotonic event sequence.
        sequence: u64,
        /// UTF-8 transcript.
        text: String,
        /// Short language code selected by Whisper.
        language: String,
    },
    /// Recognition completed without usable speech.
    NoSpeech {
        /// Monotonic event sequence.
        sequence: u64,
    },
    /// Host cancellation won the terminal race.
    Cancelled {
        /// Monotonic event sequence.
        sequence: u64,
    },
}

/// Shared host cancellation for load, inference and result decoding.
#[derive(Clone, Debug, Default)]
pub struct RecognitionCancellation(Arc<AtomicBool>);
impl RecognitionCancellation {
    /// Creates an active cancellation handle.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    /// Requests cancellation.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub(super) fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Model-neutral recognizer factory.
pub trait SpeechRecognizer {
    /// Accurately declares transcript streaming behavior.
    fn capabilities(&self) -> RecognitionCapabilities;
    /// Starts one bounded session and retains the verified model lease.
    ///
    /// # Errors
    /// Rejects unavailable resources or invalid configuration before audio acceptance.
    fn start(
        &self,
        model: SpeechModelLease,
        config: RecognitionConfig,
    ) -> Result<Box<dyn RecognitionSession>, RecognitionError>;
}

/// Bounded audio admission followed by exactly one terminal operation.
pub trait RecognitionSession: Send {
    /// Accepts the next ordered normalized frame.
    ///
    /// # Errors
    /// Rejects format changes, invalid samples, sequence gaps and resource limits.
    fn push_audio(&mut self, frame: AudioFrame) -> Result<(), RecognitionError>;
    /// Stops audio admission and performs recognition.
    ///
    /// # Errors
    /// Returns a sanitized helper, deadline or protocol error.
    fn finish(
        self: Box<Self>,
        cancellation: &RecognitionCancellation,
    ) -> Result<RecognitionEvent, RecognitionError>;
    /// Discards buffered audio without launching inference.
    fn cancel(self: Box<Self>) -> RecognitionEvent;
}

/// Sanitized recognition boundary failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecognitionError {
    /// Configuration failed validation.
    InvalidConfiguration,
    /// Frame failed shape, ordering or finite-sample validation.
    InvalidAudio,
    /// Audio exceeds 120 seconds or 16 MiB.
    AudioLimitExceeded,
    /// Helper executable or verified model is unavailable.
    Unavailable,
    /// Helper exited unsuccessfully.
    HelperFailed,
    /// Private protocol is malformed, mismatched or unbounded.
    InvalidProtocol,
    /// Recognition exceeded the host deadline.
    DeadlineExceeded,
}

impl fmt::Display for RecognitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidConfiguration => "speech recognition configuration is invalid",
            Self::InvalidAudio => "speech audio frame is invalid",
            Self::AudioLimitExceeded => "speech audio exceeds the recognition limit",
            Self::Unavailable => "speech recognition is unavailable",
            Self::HelperFailed => "the speech helper failed",
            Self::InvalidProtocol => "the speech helper returned an invalid result",
            Self::DeadlineExceeded => "speech recognition exceeded its deadline",
        })
    }
}
impl std::error::Error for RecognitionError {}

pub(super) const MAX_AUDIO_SAMPLES: usize = 120 * 16_000;
pub(super) const MAX_AUDIO_BYTES: usize = 16 * 1024 * 1024;
pub(super) const MAX_FRAME_SAMPLES: usize = 16_000;
pub(super) const RECOGNITION_DEADLINE: Duration = Duration::from_mins(1);
