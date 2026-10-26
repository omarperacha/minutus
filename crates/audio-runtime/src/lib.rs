//! Portable, bounded microphone capture for local speech features.

mod backend;
mod contract;
mod normalize;
mod session;

pub use backend::{ActiveInputStream, CpalInputBackend, InputBackend, InputSink};
pub use contract::{
    CaptureAuthorization, CaptureError, CaptureEvent, CaptureStopReason, InputLevel,
    MAX_CALLBACK_SAMPLES, MAX_CAPTURE_SAMPLES, NativeAudioChunk, NormalizedAudioFrame,
    OUTPUT_SAMPLE_RATE_HZ,
};
pub use session::{AudioCapture, AudioCaptureSession, CaptureSession, MicrophoneCapture};

/// CPAL release pinned for the portable capture backend.
pub const CPAL_VERSION: &str = "0.18.2";
