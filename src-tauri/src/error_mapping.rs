//! Sanitized translation from internal failures to the public IPC error contract.

use super::{
    AppCoreError, DownloadError, FilesystemCreateDirectoryError, FilesystemToolError,
    FilesystemWriteError, ImportError, InferenceError, IpcError, IpcErrorCode, MAX_READ_TEXT_BYTES,
    ModelOutputFailureKind, OffsetDateTime, OllamaError, Rfc3339, workspace_bookmark,
};

pub(crate) fn stream_error(error: ModelOutputFailureKind) -> IpcError {
    match error {
        ModelOutputFailureKind::MemoryPressure => IpcError {
            code: IpcErrorCode::Unsupported,
            message: "Generation stopped because system memory is low".to_owned(),
            retryable: true,
        },
        ModelOutputFailureKind::Cancelled => IpcError {
            code: IpcErrorCode::Cancelled,
            message: "Generation was stopped".to_owned(),
            retryable: true,
        },
        ModelOutputFailureKind::ProviderTimeout => IpcError {
            code: IpcErrorCode::Unsupported,
            message: "The local model took too long to respond. You can try again.".to_owned(),
            retryable: true,
        },
        ModelOutputFailureKind::ProviderUnavailable => IpcError {
            code: IpcErrorCode::Unsupported,
            message: "The local model service became unavailable. You can try again.".to_owned(),
            retryable: true,
        },
        ModelOutputFailureKind::ProviderRejected => IpcError {
            code: IpcErrorCode::Internal,
            message: "The local model service rejected the request. You can try again.".to_owned(),
            retryable: true,
        },
        ModelOutputFailureKind::InvalidProviderResponse => IpcError {
            code: IpcErrorCode::Internal,
            message: "The local model service returned an invalid response. You can try again."
                .to_owned(),
            retryable: true,
        },
        ModelOutputFailureKind::UnexpectedEnd => IpcError {
            code: IpcErrorCode::Internal,
            message: "The local model response stream ended before completion. You can try again."
                .to_owned(),
            retryable: true,
        },
        _ => IpcError {
            code: IpcErrorCode::Internal,
            message: "The model returned an invalid response. You can try again.".to_owned(),
            retryable: true,
        },
    }
}

pub(crate) fn generation_error(error: AppCoreError) -> IpcError {
    match error {
        AppCoreError::ConversationNotFound => {
            IpcError::invalid_request("The conversation is unavailable")
        }
        AppCoreError::InstalledModelNotFound => IpcError {
            code: IpcErrorCode::Unsupported,
            message: "No verified local model is installed".to_owned(),
            retryable: false,
        },
        AppCoreError::VisionProjectorNotInstalled
        | AppCoreError::Inference(InferenceError::MultimodalNotLoaded) => IpcError {
            code: IpcErrorCode::Unsupported,
            message: "Image understanding requires the optional vision projector, which is not installed for the current model.".to_owned(),
            retryable: false,
        },
        AppCoreError::WorkingContext(agent_core::WorkingContextError::ContextCapacityGap) => IpcError {
            code: IpcErrorCode::Unsupported,
            message: "Context capacity gap: the required current inputs cannot fit safely. Select a model with a larger context or narrow the current input. This conversation and its exact historical inputs remain retained for recall.".to_owned(),
            retryable: false,
        },
        AppCoreError::Inference(InferenceError::ContextLimitExceeded) => IpcError {
            code: IpcErrorCode::Unsupported,
            message: "The retained conversation exceeds the loaded model context. Earlier inputs were not silently discarded. Start a new conversation with the necessary context to continue.".to_owned(),
            retryable: false,
        },
        AppCoreError::ContextAdmission(error) if error.is_recoverable() => IpcError {
            code: IpcErrorCode::Unsupported,
            message: "The retained conversation and response reserves no longer fit the model context. Earlier inputs were not silently discarded. Start a new conversation with the necessary context to continue."
                .to_owned(),
            retryable: false,
        },
        AppCoreError::Inference(InferenceError::MemoryPressure) => IpcError {
            code: IpcErrorCode::Unsupported,
            message: "Generation could not start because system memory is low".to_owned(),
            retryable: true,
        },
        AppCoreError::ModelPrompt(_) | AppCoreError::UnsupportedConversationContent => {
            IpcError::invalid_request("The conversation cannot be rendered safely")
        }
        other => {
            let mut error = app_core_error(other);
            error.retryable = true;
            error
        }
    }
}

pub(crate) fn bookmark_access_error(error: workspace_bookmark::BookmarkAccessError) -> IpcError {
    match error {
        workspace_bookmark::BookmarkAccessError::Stale => IpcError {
            code: IpcErrorCode::Unsupported,
            message: "Workspace access needs to be re-authorized".to_owned(),
            retryable: false,
        },
        workspace_bookmark::BookmarkAccessError::Invalid
        | workspace_bookmark::BookmarkAccessError::Denied => IpcError {
            code: IpcErrorCode::Unsupported,
            message: "The workspace folder is no longer accessible".to_owned(),
            retryable: true,
        },
        #[cfg(not(target_os = "macos"))]
        workspace_bookmark::BookmarkAccessError::Unsupported => IpcError {
            code: IpcErrorCode::Unsupported,
            message: "Workspace access is unavailable on this platform".to_owned(),
            retryable: false,
        },
    }
}

pub(crate) fn filesystem_tool_error(error: &FilesystemToolError) -> IpcError {
    match error {
        FilesystemToolError::FileTooLarge => IpcError::payload_too_large(format!(
            "The workspace file exceeds the limit of {MAX_READ_TEXT_BYTES} bytes"
        )),
        FilesystemToolError::InvalidUtf8 => IpcError {
            code: IpcErrorCode::Unsupported,
            message: "The workspace file is not valid UTF-8 text".to_owned(),
            retryable: false,
        },
        FilesystemToolError::BinaryFile => IpcError {
            code: IpcErrorCode::Unsupported,
            message: "The workspace file is binary".to_owned(),
            retryable: false,
        },
        FilesystemToolError::InvalidQuery => {
            IpcError::invalid_request("The filesystem discovery or search query is invalid")
        }
        FilesystemToolError::InvalidScanFilter => {
            IpcError::invalid_request("The filesystem scan filter is invalid")
        }
        FilesystemToolError::ChangedDuringRead => IpcError {
            code: IpcErrorCode::Unsupported,
            message: "The workspace file changed while it was being read".to_owned(),
            retryable: true,
        },
        FilesystemToolError::Path(filesystem_tools::SafePathError::ProtectedPath) => {
            protected_path_error()
        }
        FilesystemToolError::Path(_) => {
            IpcError::invalid_request("The workspace path is unavailable or unsafe")
        }
        FilesystemToolError::Io(_) => IpcError {
            code: IpcErrorCode::Internal,
            message: "The workspace could not be read".to_owned(),
            retryable: true,
        },
    }
}

pub(crate) fn filesystem_write_error(error: &FilesystemWriteError) -> IpcError {
    match error {
        FilesystemWriteError::LimitExceeded => {
            IpcError::payload_too_large("The workspace write exceeds a safety limit")
        }
        FilesystemWriteError::InvalidDigest => {
            IpcError::invalid_request("expectedSha256 must be a lowercase SHA-256 digest")
        }
        FilesystemWriteError::Conflict => IpcError {
            code: IpcErrorCode::Unsupported,
            message: "Workspace content changed; read it again before writing".to_owned(),
            retryable: true,
        },
        FilesystemWriteError::Path(filesystem_tools::SafePathError::ProtectedPath) => {
            protected_path_error()
        }
        FilesystemWriteError::Path(_) => {
            IpcError::invalid_request("The workspace path is unavailable or unsafe")
        }
        FilesystemWriteError::Io(_) | FilesystemWriteError::PostconditionFailed => IpcError {
            code: IpcErrorCode::Internal,
            message: "The workspace could not be updated".to_owned(),
            retryable: true,
        },
    }
}

fn filesystem_create_directory_error(error: &FilesystemCreateDirectoryError) -> IpcError {
    match error {
        FilesystemCreateDirectoryError::Conflict => IpcError {
            code: IpcErrorCode::Unsupported,
            message: "The workspace directory already exists".to_owned(),
            retryable: false,
        },
        FilesystemCreateDirectoryError::Path(filesystem_tools::SafePathError::ProtectedPath) => {
            protected_path_error()
        }
        FilesystemCreateDirectoryError::Path(_) => {
            IpcError::invalid_request("The workspace path is unavailable or unsafe")
        }
        FilesystemCreateDirectoryError::Io(_) => IpcError {
            code: IpcErrorCode::Internal,
            message: "The workspace directory could not be created".to_owned(),
            retryable: true,
        },
    }
}

fn protected_path_error() -> IpcError {
    IpcError::permission_denied(
        "Credential files and protected configuration paths are unavailable to workspace tools",
    )
}

fn speech_model_error(error: model_runtime::asr::SpeechModelError) -> IpcError {
    match error {
        model_runtime::asr::SpeechModelError::Download(error) => {
            app_core_error(AppCoreError::ModelDownload(error))
        }
        model_runtime::asr::SpeechModelError::Busy => IpcError {
            code: IpcErrorCode::Unsupported,
            message: "The speech model is in use. Finish or cancel the current operation first."
                .to_owned(),
            retryable: true,
        },
    }
}

fn ollama_error(error: OllamaError) -> IpcError {
    match error {
        OllamaError::Cancelled => IpcError {
            code: IpcErrorCode::Cancelled,
            message: "The Ollama model installation was cancelled".to_owned(),
            retryable: true,
        },
        OllamaError::Unavailable | OllamaError::DeadlineExceeded => {
            IpcError {
                code: IpcErrorCode::Unsupported,
                message: "Minutus's local inference service is temporarily unavailable. Minutus will restart it automatically; retry the request."
                    .to_owned(),
                retryable: true,
            }
        }
        OllamaError::IncompatibleServer => IpcError {
            code: IpcErrorCode::Unsupported,
            message: "The installed Ollama version is not compatible with this model.".to_owned(),
            retryable: false,
        },
        OllamaError::ModelUnavailable => IpcError {
            code: IpcErrorCode::Unsupported,
            message: "Ollama did not register the exact pinned model digest.".to_owned(),
            retryable: true,
        },
        _ => IpcError {
            code: IpcErrorCode::Internal,
            message: "The local Ollama service could not complete the request.".to_owned(),
            retryable: true,
        },
    }
}

pub(crate) fn app_core_error(error: AppCoreError) -> IpcError {
    match error {
        AppCoreError::ExternalModel(error) => IpcError::invalid_request(error.to_string()),
        error @ (AppCoreError::ForegroundActivityBusy
        | AppCoreError::InvalidDictationBinding
        | AppCoreError::DictationUnavailable
        | AppCoreError::SpeechModelNotInstalled) => dictation_error(&error),
        AppCoreError::CatalogueModelNotFound => {
            IpcError::invalid_request("The requested model release is unavailable")
        }
        AppCoreError::InvalidCompanionSelection => {
            IpcError::invalid_request("The selected model companions are unavailable")
        }
        AppCoreError::SpeechModel(error) => speech_model_error(error),
        AppCoreError::ModelDownload(DownloadError::Cancelled) => IpcError {
            code: IpcErrorCode::Cancelled,
            message: "The model download was cancelled".to_owned(),
            retryable: true,
        },
        AppCoreError::ModelDownload(DownloadError::InsufficientDiskSpace {
            required,
            available,
        })
        | AppCoreError::ModelImport(ImportError::InsufficientDiskSpace {
            required,
            available,
        }) => IpcError {
            code: IpcErrorCode::Unsupported,
            message: format!(
                "Not enough disk space: {required} bytes required, {available} bytes available"
            ),
            retryable: true,
        },
        AppCoreError::ModelDownload(error) => IpcError {
            code: IpcErrorCode::Internal,
            message: error.to_string(),
            retryable: true,
        },
        AppCoreError::OllamaSidecar(_) | AppCoreError::OllamaServiceUnavailable => IpcError {
            code: IpcErrorCode::Unsupported,
            message: "Minutus's bundled inference service could not start. Retry the request or restart Minutus."
                .to_owned(),
            retryable: true,
        },
        AppCoreError::Ollama(error) => ollama_error(error),
        AppCoreError::ModelImport(
            error @ (ImportError::Gguf(_)
            | ImportError::SourceNotRegularFile
            | ImportError::SourceChanged),
        ) => IpcError {
            code: IpcErrorCode::Unsupported,
            message: error.to_string(),
            retryable: false,
        },
        AppCoreError::ModelImport(ImportError::Source(_)) => IpcError {
            code: IpcErrorCode::Unsupported,
            message: "The selected GGUF could not be read".to_owned(),
            retryable: true,
        },
        AppCoreError::WorkspaceGrantNotFound => {
            IpcError::invalid_request("The workspace grant is unavailable")
        }
        AppCoreError::InvalidProjectName => {
            IpcError::invalid_request("The project name is invalid")
        }
        AppCoreError::WorkspaceAuthorization(_) => IpcError {
            code: IpcErrorCode::Unsupported,
            message: "The workspace permission does not allow this operation".to_owned(),
            retryable: false,
        },
        AppCoreError::Filesystem(error) => filesystem_tool_error(&error),
        AppCoreError::FilesystemWrite(error) => filesystem_write_error(&error),
        AppCoreError::FilesystemCreateDirectory(error) => filesystem_create_directory_error(&error),
        _ => IpcError::internal(),
    }
}

fn dictation_error(error: &AppCoreError) -> IpcError {
    match error {
        AppCoreError::ForegroundActivityBusy => IpcError {
            code: IpcErrorCode::Unsupported,
            message: "Finish the active response or dictation session first.".to_owned(),
            retryable: true,
        },
        AppCoreError::InvalidDictationBinding => {
            IpcError::invalid_request("The dictation session does not match this composer")
        }
        AppCoreError::DictationUnavailable => IpcError {
            code: IpcErrorCode::Internal,
            message: "Dictation is unavailable".to_owned(),
            retryable: true,
        },
        AppCoreError::SpeechModelNotInstalled => IpcError {
            code: IpcErrorCode::Unsupported,
            message: "Download the local speech model in Settings before dictating.".to_owned(),
            retryable: true,
        },
        _ => IpcError::internal(),
    }
}

pub(crate) fn current_timestamp() -> Result<String, IpcError> {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|_| IpcError::internal())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_multimodal_runtime_has_an_actionable_generation_error() {
        for error in [
            AppCoreError::VisionProjectorNotInstalled,
            AppCoreError::Inference(InferenceError::MultimodalNotLoaded),
        ] {
            let mapped = generation_error(error);
            assert_eq!(mapped.code, IpcErrorCode::Unsupported);
            assert!(mapped.message.contains("vision projector"));
            assert!(!mapped.retryable);
        }
    }

    #[test]
    fn ollama_unavailability_never_asks_for_an_external_application() {
        let mapped = app_core_error(AppCoreError::Ollama(OllamaError::Unavailable));
        assert!(mapped.message.contains("Minutus"));
        assert!(mapped.message.contains("automatically"));
        assert!(!mapped.message.contains("Start Ollama"));
        assert!(mapped.retryable);
    }

    #[test]
    fn stream_provider_failures_have_actionable_distinct_messages() {
        let timeout = stream_error(ModelOutputFailureKind::ProviderTimeout);
        assert!(timeout.message.contains("too long"));
        assert!(timeout.retryable);

        let unavailable = stream_error(ModelOutputFailureKind::ProviderUnavailable);
        assert!(unavailable.message.contains("became unavailable"));
        assert!(unavailable.retryable);

        let invalid = stream_error(ModelOutputFailureKind::InvalidProviderResponse);
        assert!(invalid.message.contains("invalid response"));
        assert!(!invalid.message.contains("Ollama"));

        let incomplete = stream_error(ModelOutputFailureKind::UnexpectedEnd);
        assert!(incomplete.message.contains("ended before completion"));
        assert!(!incomplete.message.contains("unavailable"));
    }
}

#[cfg(test)]
mod context_gap_tests;
