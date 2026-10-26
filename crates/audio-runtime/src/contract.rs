use std::fmt;

/// Maximum source samples admitted from one real-time callback.
pub const MAX_CALLBACK_SAMPLES: usize = 262_144;
/// Maximum normalized capture duration.
pub const MAX_CAPTURE_SAMPLES: usize = 120 * 16_000;
/// Whisper-compatible output sample rate.
pub const OUTPUT_SAMPLE_RATE_HZ: u32 = 16_000;

/// Proof that capture startup follows an explicit user action.
#[derive(Debug)]
pub struct CaptureAuthorization {
    _private: (),
}

impl CaptureAuthorization {
    /// Marks a capture start requested directly by the user. The native backend
    /// still verifies or presents platform permission before audio can arrive.
    #[must_use]
    pub const fn after_user_action() -> Self {
        Self { _private: () }
    }
}

/// Portable native input copied from an audio callback.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeAudioChunk {
    /// Device sample rate.
    pub sample_rate_hz: u32,
    /// Interleaved channel count.
    pub channels: u16,
    /// Interleaved normalized samples.
    pub samples: Vec<f32>,
}

/// One ordered 16 kHz mono f32 output frame.
#[derive(Clone, Debug, PartialEq)]
pub struct NormalizedAudioFrame {
    /// Zero-based frame sequence.
    pub sequence: u64,
    /// Mono samples in `[-1, 1]`.
    pub samples: Vec<f32>,
}

/// Root-mean-square input level suitable for a meter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InputLevel(f32);

impl InputLevel {
    /// Silent input level for portable capture backends and tests.
    #[must_use]
    pub const fn silence() -> Self {
        Self(0.0)
    }

    pub(crate) fn from_samples(samples: &[f32]) -> Self {
        if samples.is_empty() {
            return Self::silence();
        }
        let sum = samples
            .iter()
            .map(|sample| f64::from(*sample) * f64::from(*sample))
            .sum::<f64>();
        let count = samples.iter().fold(0.0_f64, |value, _| value + 1.0);
        #[allow(clippy::cast_possible_truncation)]
        // The square root is bounded to 0..=1 before conversion by validated samples.
        let rms = (sum / count).sqrt() as f32;
        Self(rms.clamp(0.0, 1.0))
    }

    /// Returns a finite value in `0..=1`.
    #[must_use]
    pub const fn value(self) -> f32 {
        self.0
    }
}

/// Ordered capture output for a non-blocking consumer.
#[derive(Clone, Debug, PartialEq)]
pub enum CaptureEvent {
    /// Normalized audio and its meter value.
    Frame {
        /// Audio frame.
        frame: NormalizedAudioFrame,
        /// Level derived from this frame.
        level: InputLevel,
    },
    /// Capture stopped without an audio failure.
    Stopped(CaptureStopReason),
    /// Capture failed and the native stream has been released.
    Failed(CaptureError),
}

/// Host-directed or bounded automatic stop reason.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureStopReason {
    /// The user finished recording.
    Finished,
    /// The user cancelled recording.
    Cancelled,
    /// Navigation disposed the initiating composer.
    Navigation,
    /// The application window closed.
    WindowClosed,
    /// The system slept or suspended long enough to invalidate capture.
    SystemSleep,
    /// The 120-second normalized duration limit was reached.
    DurationLimit,
}

/// Sanitized microphone boundary failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureError {
    /// Platform permission has not been granted or was revoked.
    PermissionDenied,
    /// No default input device is available.
    NoInputDevice,
    /// The device disappeared or invalidated the active stream.
    DeviceLost,
    /// The device's format cannot be safely normalized.
    UnsupportedFormat,
    /// The stream could not start.
    StartFailed,
    /// A real-time callback or consumer exceeded a bounded queue.
    Overflow,
}

impl fmt::Display for CaptureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::PermissionDenied => "microphone access is denied; enable it in system settings",
            Self::NoInputDevice => "no microphone input device is available",
            Self::DeviceLost => "the microphone became unavailable",
            Self::UnsupportedFormat => "the microphone format is unsupported",
            Self::StartFailed => "the microphone could not start",
            Self::Overflow => "microphone audio arrived faster than it could be processed",
        })
    }
}

impl std::error::Error for CaptureError {}
