use std::{error::Error, fmt};

use crate::{GgufError, ModelOutputFailureKind, SidecarError};

/// Failure while verifying, loading or running local inference.
#[derive(Debug)]
pub enum InferenceError {
    /// The runtime lock is poisoned and cannot be trusted.
    RuntimeUnavailable,
    /// A managed artifact path resolves through a symbolic link.
    ArtifactNotRegularFile,
    /// The bundled helper is missing or invalid.
    InvalidExecutable,
    /// The expected digest is not canonical lowercase SHA-256.
    InvalidDigest,
    /// The managed artifact size does not match its persisted identity.
    ArtifactSizeMismatch,
    /// The managed artifact changed while it was being verified.
    ArtifactChanged,
    /// Full hashing did not match the persisted digest.
    ArtifactDigestMismatch,
    /// The requested context/output relationship is invalid.
    InvalidLimits,
    /// The manifest requires a different `llama.cpp` build.
    IncompatibleBackend,
    /// Selected companions do not belong to the primary model's exact compatibility group.
    IncompatibleCompanion,
    /// A profile selected more than one artifact for the same companion role.
    DuplicateCompanionKind,
    /// The prompt is empty or exceeds its byte bound.
    InvalidPrompt,
    /// Image input was requested without a verified vision projector profile.
    MultimodalNotLoaded,
    /// A generation exceeds the profile's image-count limit.
    ImageCountExceeded,
    /// Image bytes are not a bounded normalized PNG derivative.
    InvalidImageInput,
    /// Prompt image markers and supplied image bytes do not correspond exactly.
    ImageMarkerMismatch,
    /// No verified model is resident.
    ModelNotLoaded,
    /// Another generation already owns the single inference slot.
    GenerationAlreadyActive,
    /// Admission or generation used a stale model/adapter/tokenizer/provider binding.
    ProfileBindingMismatch,
    /// Requested output exceeds the loaded profile.
    OutputLimitExceeded,
    /// Prompt plus output exceeds the loaded context.
    ContextLimitExceeded,
    /// A failed helper must be restarted before inference.
    RestartRequired,
    /// No failed loaded model is eligible for restart.
    RestartNotAvailable,
    /// The rolling restart bound has been exhausted.
    RestartLimitExceeded,
    /// The helper exited unexpectedly.
    SidecarExited,
    /// The helper did not become healthy within its bounded load window.
    LoadTimedOut,
    /// Generation was explicitly cancelled.
    Cancelled,
    /// Memory pressure prevented or interrupted inference.
    MemoryPressure,
    /// Cancellation interrupted the active streaming request.
    GenerationInterrupted,
    /// Glimmer emitted malformed output.
    MalformedModelOutput,
    /// The stream ended without a terminal server frame.
    UnexpectedStreamEnd,
    /// One SSE event exceeded its fixed bound.
    StreamFrameTooLarge,
    /// The SSE framing or UTF-8 was malformed.
    MalformedStream,
    /// A bounded control response exceeded its limit.
    ControlResponseTooLarge,
    /// A helper control response was malformed.
    MalformedServerResponse,
    /// The helper rejected an authenticated request.
    ServerRejected,
    /// An API credential could not be represented as an HTTP header.
    InvalidCredential,
    /// Model artifact I/O failed.
    ArtifactIo(std::io::Error),
    /// GGUF compatibility inspection failed.
    Gguf(GgufError),
    /// Sidecar supervision failed.
    Sidecar(SidecarError),
    /// HTTP client construction failed.
    Client(reqwest::Error),
    /// An authenticated loopback request failed.
    Server(reqwest::Error),
    /// Reading the bounded server stream failed.
    StreamIo(std::io::Error),
    /// The selected model-output adapter rejected the request scope.
    Adapter(ModelOutputFailureKind),
}

impl fmt::Display for InferenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RuntimeUnavailable => formatter.write_str("the inference runtime is unavailable"),
            Self::ArtifactNotRegularFile => {
                formatter.write_str("the managed model path is invalid")
            }
            Self::InvalidExecutable => {
                formatter.write_str("the bundled inference helper is invalid")
            }
            Self::InvalidDigest => formatter.write_str("the managed model digest is invalid"),
            Self::ArtifactSizeMismatch => {
                formatter.write_str("the managed model size does not match")
            }
            Self::ArtifactChanged => {
                formatter.write_str("the managed model changed during verification")
            }
            Self::ArtifactDigestMismatch => {
                formatter.write_str("the managed model failed integrity verification")
            }
            Self::InvalidLimits => formatter.write_str("the inference limits are invalid"),
            Self::IncompatibleBackend => {
                formatter.write_str("the model requires an incompatible inference backend")
            }
            Self::IncompatibleCompanion => {
                formatter.write_str("a companion is incompatible with the selected model")
            }
            Self::DuplicateCompanionKind => {
                formatter.write_str("the inference profile contains a duplicate companion role")
            }
            Self::InvalidPrompt => formatter.write_str("the inference prompt is invalid"),
            Self::MultimodalNotLoaded => {
                formatter.write_str("image input requires the verified vision projector")
            }
            Self::ImageCountExceeded => formatter.write_str("the request contains too many images"),
            Self::InvalidImageInput => {
                formatter.write_str("an image is not a supported validated local asset")
            }
            Self::ImageMarkerMismatch => {
                formatter.write_str("image attachments do not match the rendered prompt")
            }
            Self::ModelNotLoaded => formatter.write_str("no verified model is loaded"),
            Self::GenerationAlreadyActive => formatter.write_str("a generation is already active"),
            Self::ProfileBindingMismatch => {
                formatter.write_str("the generation profile binding is stale")
            }
            Self::OutputLimitExceeded => {
                formatter.write_str("the generation output limit is too large")
            }
            Self::ContextLimitExceeded => {
                formatter.write_str("the generation exceeds the model context limit")
            }
            Self::RestartRequired => formatter.write_str("the inference helper must be restarted"),
            Self::RestartNotAvailable => formatter.write_str("inference restart is not available"),
            Self::RestartLimitExceeded => {
                formatter.write_str("the inference restart limit was reached")
            }
            Self::SidecarExited => formatter.write_str("the inference helper exited unexpectedly"),
            Self::LoadTimedOut => formatter.write_str("the inference helper did not become ready"),
            Self::Cancelled => formatter.write_str("generation was cancelled"),
            Self::MemoryPressure => {
                formatter.write_str("inference stopped because system memory is low")
            }
            Self::GenerationInterrupted => formatter.write_str("generation was interrupted"),
            Self::MalformedModelOutput => formatter.write_str("the model produced invalid output"),
            Self::UnexpectedStreamEnd => {
                formatter.write_str("the inference stream ended unexpectedly")
            }
            Self::StreamFrameTooLarge => {
                formatter.write_str("an inference stream frame is too large")
            }
            Self::MalformedStream => formatter.write_str("the inference stream is malformed"),
            Self::ControlResponseTooLarge => {
                formatter.write_str("an inference control response is too large")
            }
            Self::MalformedServerResponse => {
                formatter.write_str("the inference helper response is malformed")
            }
            Self::ServerRejected => {
                formatter.write_str("the inference helper rejected the request")
            }
            Self::InvalidCredential => formatter.write_str("the inference credential is invalid"),
            Self::ArtifactIo(_) => formatter.write_str("the managed model could not be read"),
            Self::Gguf(error) => error.fmt(formatter),
            Self::Sidecar(error) => error.fmt(formatter),
            Self::Client(_) => formatter.write_str("the inference client could not be initialized"),
            Self::Server(_) => formatter.write_str("the local inference helper is unavailable"),
            Self::StreamIo(_) => formatter.write_str("the local inference stream failed"),
            Self::Adapter(_) => formatter.write_str("the model output adapter rejected the stream"),
        }
    }
}

impl Error for InferenceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ArtifactIo(error) | Self::StreamIo(error) => Some(error),
            Self::Gguf(error) => Some(error),
            Self::Sidecar(error) => Some(error),
            Self::Client(error) | Self::Server(error) => Some(error),
            _ => None,
        }
    }
}
