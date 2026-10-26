//! Speech setup values. No audio, native paths or microphone authority cross IPC.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{ConversationId, DictationSessionId, ProjectId};

/// Exact composer instance and draft revision that may receive a transcript.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DictationBinding {
    /// Active project, including the project of an unpersisted new conversation.
    pub project_id: ProjectId,
    /// Existing conversation, absent for the empty/new-conversation composer.
    pub conversation_id: Option<ConversationId>,
    /// Frontend-generated identity for one mounted composer instance.
    pub composer_id: String,
    /// Monotonic draft revision captured before recording begins.
    #[ts(type = "number")]
    pub draft_revision: u64,
}

/// Explicit user request to begin local dictation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartDictationRequest {
    /// Composer authorized to receive the result.
    pub binding: DictationBinding,
    /// Exact installed speech profile.
    pub model: SpeechModelRequest,
    /// Optional canonical spoken-language hint; absent enables detection.
    pub language: Option<String>,
}

/// Accepted host-owned session identity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DictationAccepted {
    /// New opaque session identifier.
    pub session_id: DictationSessionId,
    /// Exact binding retained by the host.
    pub binding: DictationBinding,
}

/// Binding required for finish and cancellation commands.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DictationCommand {
    /// Target session.
    pub session_id: DictationSessionId,
    /// Exact initiating composer and revision.
    pub binding: DictationBinding,
}

/// Bounded event polling request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DictationEventsRequest {
    /// Target session.
    pub session_id: DictationSessionId,
    /// Exact initiating composer and revision.
    pub binding: DictationBinding,
    /// Last sequence already applied, or zero before the first event.
    #[ts(type = "number")]
    pub after_sequence: u64,
}

/// Stable frontend-safe dictation failure category.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum DictationFailureCode {
    /// Microphone permission was denied or revoked.
    PermissionDenied,
    /// No usable default microphone exists.
    NoInputDevice,
    /// The active microphone disappeared.
    DeviceLost,
    /// A bounded audio queue or buffer overflowed.
    AudioOverflow,
    /// Installed speech weights are missing or incompatible.
    SpeechModelUnavailable,
    /// The local recognition helper could not be launched.
    RecognitionUnavailable,
    /// Recognition rejected audio or returned an invalid result.
    RecognitionFailed,
    /// Recognition exceeded its host deadline.
    DeadlineExceeded,
    /// A sanitized internal capture or coordination failure.
    Internal,
}

/// One ordered state transition. Terminal variants appear at most once.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum DictationEventKind {
    /// Verifying the model and preparing recognition.
    Preparing,
    /// Microphone capture is active.
    Recording {
        /// RMS input level scaled to `0..=1000`.
        level_millipercent: u16,
        /// Host-observed recording duration.
        #[ts(type = "number")]
        elapsed_ms: u64,
    },
    /// Capture has stopped and final-only recognition is running.
    Transcribing {
        /// The capture duration limit ended recording automatically.
        duration_limit_reached: bool,
    },
    /// Terminal editable transcript.
    Completed {
        /// Bounded editable transcript; never submitted automatically.
        text: String,
        /// Bounded detected or requested spoken language.
        language: String,
    },
    /// Terminal result indicating no usable speech.
    NoSpeech,
    /// Terminal host or user cancellation.
    Cancelled,
    /// Terminal sanitized failure.
    Failed {
        /// Stable handling category.
        code: DictationFailureCode,
        /// Sanitized actionable copy.
        message: String,
    },
}

impl DictationEventKind {
    /// Whether this event permanently ends the session.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed { .. } | Self::NoSpeech | Self::Cancelled | Self::Failed { .. }
        )
    }
}

/// Ordered event bound to one session and one composer revision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DictationEvent {
    /// Session that owns this event.
    pub session_id: DictationSessionId,
    /// Exact composer binding retained by the host.
    pub binding: DictationBinding,
    /// Monotonic sequence within this session.
    #[ts(type = "number")]
    pub sequence: u64,
    /// State transition or terminal result.
    pub event: DictationEventKind,
}

/// Exact speech-only release requested for installation or removal.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpeechModelRequest {
    /// Publisher-qualified identity from the speech catalogue.
    pub model_id: String,
    /// Immutable revision from the speech catalogue.
    pub model_version: String,
}

impl crate::ValidateIpc for SpeechModelRequest {
    fn validate(&self) -> Result<(), crate::IpcError> {
        for value in [&self.model_id, &self.model_version] {
            if value.is_empty() || value.len() > 128 || value.trim() != value || !value.is_ascii() {
                return Err(crate::IpcError::invalid_request(
                    "The speech model selection is invalid",
                ));
            }
        }
        Ok(())
    }
}

/// Latest bounded native speech download snapshot.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpeechDownloadProgress {
    /// Current native installation phase.
    pub phase: crate::ModelDownloadPhase,
    /// Bytes retained in the candidate.
    #[ts(type = "number")]
    pub downloaded_bytes: u64,
    /// Exact expected artifact size.
    #[ts(type = "number")]
    pub total_bytes: u64,
    /// Whether this attempt reused retained partial bytes.
    pub resumed: bool,
}

/// Mutually exclusive speech installation states.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum SpeechInstallState {
    /// No verified weights; download or resume is available.
    Missing,
    /// Complete artifact passed local integrity and compatibility checks.
    Installed,
    /// Remove the changed or unsupported artifact and download again.
    Incompatible,
    /// A host-owned transfer remains active even if settings are closed.
    Downloading {
        /// Latest snapshot; absent while preparing the download.
        progress: Option<SpeechDownloadProgress>,
    },
    /// Weights are currently leased or another storage operation is active.
    InUse,
}

/// Optional speech setup profile; never a chat-model selection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpeechModelView {
    /// Exact profile selection.
    pub selection: SpeechModelRequest,
    /// Human-readable model name.
    pub display_name: String,
    /// Exact download size shown before installation.
    #[ts(type = "number")]
    pub download_bytes: u64,
    /// Publisher licence URL.
    pub license_url: String,
    /// Current host-owned installation state.
    pub state: SpeechInstallState,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ValidateIpc;

    fn request() -> StartDictationRequest {
        StartDictationRequest {
            binding: DictationBinding {
                project_id: ProjectId("project".to_owned()),
                conversation_id: None,
                composer_id: "composer-1".to_owned(),
                draft_revision: 4,
            },
            model: SpeechModelRequest {
                model_id: "whisper".to_owned(),
                model_version: "v1".to_owned(),
            },
            language: Some("en".to_owned()),
        }
    }

    #[test]
    fn dictation_inputs_bind_and_bound_frontend_identity() {
        assert!(request().validate().is_ok());
        let mut invalid = request();
        invalid.binding.composer_id = " ".to_owned();
        assert!(invalid.validate().is_err());
        let mut invalid = request();
        invalid.language = Some("EN_us".to_owned());
        assert!(invalid.validate().is_err());
    }
}
