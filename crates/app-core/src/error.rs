use super::{
    AgentSystemInstructionError, AssetStoreError, CatalogueError, ContextAdmissionError,
    DiagnosticExportError, DownloadError, Error, FilesystemCreateDirectoryError,
    FilesystemToolError, FilesystemWriteError, ImportError, InferenceError, OllamaError,
    OllamaSidecarError, ProjectComputationInputsError, PromptAdapterError, StorageError,
    TokenizerError, TransactionAuditError, WorkingContextError, WorkspaceAuthorizationError, fmt,
};

/// Failure while initializing or using native application services.
#[derive(Debug)]
pub enum AppCoreError {
    /// Application-data files could not be prepared.
    Io(std::io::Error),
    /// The ordinary application database failed safely.
    Storage(StorageError),
    /// The bundled model catalogue failed strict validation.
    ModelCatalogue(CatalogueError),
    /// A requested model release is not present in the validated catalogue.
    CatalogueModelNotFound,
    /// Companion identifiers are unknown, duplicated or incompatible.
    InvalidCompanionSelection,
    /// Model download, verification or publication failed safely.
    ModelDownload(DownloadError),
    /// Speech installation or verified-use admission failed.
    SpeechModel(model_runtime::asr::SpeechModelError),
    /// No complete local speech-model candidate is available for capture.
    SpeechModelNotInstalled,
    /// Chat generation or dictation already owns foreground inference admission.
    ForegroundActivityBusy,
    /// Dictation coordination state could not be accessed safely.
    DictationUnavailable,
    /// A session command did not match its initiating composer binding.
    InvalidDictationBinding,
    /// The bounded local Ollama provider failed safely.
    Ollama(OllamaError),
    /// The bundled Ollama lifecycle supervisor failed safely.
    OllamaSidecar(OllamaSidecarError),
    /// Another thread failed while holding the Ollama service lock.
    OllamaServiceUnavailable,
    /// GGUF inspection, managed copying or verification failed safely.
    ModelImport(ImportError),
    /// External model registration or reverification failed.
    ExternalModel(model_runtime::ExternalModelError),
    /// Exact vocabulary-only tokenization failed safely.
    Tokenizer(TokenizerError),
    /// The requested installed model record does not exist.
    InstalledModelNotFound,
    /// Persisted model metadata does not name a managed published artifact.
    InstalledModelInvalid,
    /// Image understanding was requested without an installed vision projector.
    VisionProjectorNotInstalled,
    /// Context or output tokens exceed the current product profile.
    InvalidInferenceLimits,
    /// Verified local inference lifecycle control failed safely.
    Inference(InferenceError),
    /// Another thread failed while holding the storage lock.
    StorageUnavailable,
    /// The requested project does not exist.
    ProjectNotFound,
    /// A user-visible project name is empty, untrimmed or too long.
    InvalidProjectName,
    /// A selected workspace is not a valid durable folder capability.
    InvalidWorkspaceSelection,
    /// The requested workspace grant is revoked or absent.
    WorkspaceGrantNotFound,
    /// The active grant level or approval state rejected an operation.
    WorkspaceAuthorization(WorkspaceAuthorizationError),
    /// Another thread failed while holding the workspace-operation lock.
    WorkspaceOperationsUnavailable,
    /// A bounded workspace filesystem operation failed safely.
    Filesystem(FilesystemToolError),
    /// Read-only project computation input binding failed safely.
    ProjectComputation(ProjectComputationInputsError),
    /// A bounded direct workspace write failed safely.
    FilesystemWrite(FilesystemWriteError),
    /// An exact workspace directory creation failed safely.
    FilesystemCreateDirectory(FilesystemCreateDirectoryError),
    /// Another thread failed while holding the asset lifecycle lock.
    AssetOperationsUnavailable,
    /// A requested opaque asset identifier is not committed.
    AssetNotFound,
    /// The requested conversation does not exist.
    ConversationNotFound,
    /// Persisted conversation content is not supported by the active chat adapter.
    UnsupportedConversationContent,
    /// Strict model prompt rendering rejected the conversation.
    ModelPrompt(PromptAdapterError),
    /// Trusted agent system-instruction assembly failed safely.
    AgentSystemInstructions(AgentSystemInstructionError),
    /// Bounded working-context construction failed safely.
    WorkingContext(WorkingContextError),
    /// Exact trusted context admission rejected the model pass.
    ContextAdmission(ContextAdmissionError),
    /// Image validation, normalization or managed-file storage failed safely.
    AssetStore(AssetStoreError),
    /// A validated web response could not enter the remote-image handoff.
    RemoteImageHandoff(web_retrieval::RemoteImageHandoffError),
    /// An explicit diagnostic export failed safely.
    DiagnosticExport(DiagnosticExportError),
    /// An explicit transaction audit export or deletion failed safely.
    TransactionAudit(TransactionAuditError),
}

impl fmt::Display for AppCoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(_) => formatter.write_str("application data could not be prepared"),
            Self::Storage(error) => error.fmt(formatter),
            Self::ModelCatalogue(error) => error.fmt(formatter),
            Self::CatalogueModelNotFound => {
                formatter.write_str("the requested catalogue model was not found")
            }
            Self::InvalidCompanionSelection => {
                formatter.write_str("the selected model companions are invalid")
            }
            Self::ModelDownload(error) => error.fmt(formatter),
            Self::SpeechModel(error) => error.fmt(formatter),
            Self::SpeechModelNotInstalled => {
                formatter.write_str("the local speech model is not installed")
            }
            Self::ForegroundActivityBusy => {
                formatter.write_str("chat generation or dictation is already active")
            }
            Self::DictationUnavailable => formatter.write_str("dictation is unavailable"),
            Self::InvalidDictationBinding => {
                formatter.write_str("the dictation session does not match this composer")
            }
            Self::Ollama(error) => error.fmt(formatter),
            Self::OllamaSidecar(error) => error.fmt(formatter),
            Self::OllamaServiceUnavailable => {
                formatter.write_str("the local Ollama service is unavailable")
            }
            Self::ModelImport(error) => error.fmt(formatter),
            Self::ExternalModel(error) => error.fmt(formatter),
            Self::Tokenizer(error) => error.fmt(formatter),
            Self::InstalledModelNotFound => {
                formatter.write_str("the installed model was not found")
            }
            Self::InstalledModelInvalid => formatter.write_str("the installed model is invalid"),
            Self::VisionProjectorNotInstalled => {
                formatter.write_str("the vision projector is not installed")
            }
            Self::InvalidInferenceLimits => formatter.write_str("the inference limits are invalid"),
            Self::Inference(error) => error.fmt(formatter),
            Self::StorageUnavailable => formatter.write_str("application storage is unavailable"),
            Self::ProjectNotFound => formatter.write_str("project was not found"),
            Self::InvalidProjectName => formatter.write_str("the project name is invalid"),
            Self::InvalidWorkspaceSelection => {
                formatter.write_str("the selected workspace is unavailable")
            }
            Self::WorkspaceGrantNotFound => formatter.write_str("workspace grant was not found"),
            Self::WorkspaceAuthorization(error) => error.fmt(formatter),
            Self::WorkspaceOperationsUnavailable => {
                formatter.write_str("workspace operations are unavailable")
            }
            Self::Filesystem(error) => error.fmt(formatter),
            Self::ProjectComputation(error) => error.fmt(formatter),
            Self::FilesystemWrite(error) => error.fmt(formatter),
            Self::FilesystemCreateDirectory(error) => error.fmt(formatter),
            Self::AssetOperationsUnavailable => {
                formatter.write_str("asset operations are unavailable")
            }
            Self::AssetNotFound => formatter.write_str("managed asset was not found"),
            Self::ConversationNotFound => formatter.write_str("conversation was not found"),
            Self::UnsupportedConversationContent => {
                formatter.write_str("conversation contains unsupported content")
            }
            Self::ModelPrompt(error) => error.fmt(formatter),
            Self::AgentSystemInstructions(error) => error.fmt(formatter),
            Self::WorkingContext(error) => error.fmt(formatter),
            Self::ContextAdmission(error) => error.fmt(formatter),
            Self::AssetStore(error) => error.fmt(formatter),
            Self::RemoteImageHandoff(error) => error.fmt(formatter),
            Self::DiagnosticExport(error) => error.fmt(formatter),
            Self::TransactionAudit(error) => error.fmt(formatter),
        }
    }
}

impl Error for AppCoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Storage(error) => Some(error),
            Self::ModelCatalogue(error) => Some(error),
            Self::ModelDownload(error) => Some(error),
            Self::SpeechModel(error) => Some(error),
            Self::Ollama(error) => Some(error),
            Self::OllamaSidecar(error) => Some(error),
            Self::ModelImport(error) => Some(error),
            Self::ExternalModel(error) => Some(error),
            Self::Tokenizer(error) => Some(error),
            Self::Inference(error) => Some(error),
            Self::ModelPrompt(error) => Some(error),
            Self::AgentSystemInstructions(error) => Some(error),
            Self::WorkingContext(error) => Some(error),
            Self::ContextAdmission(error) => Some(error),
            Self::AssetStore(error) => Some(error),
            Self::RemoteImageHandoff(error) => Some(error),
            Self::Filesystem(error) => Some(error),
            Self::ProjectComputation(error) => Some(error),
            Self::FilesystemWrite(error) => Some(error),
            Self::FilesystemCreateDirectory(error) => Some(error),
            Self::CatalogueModelNotFound
            | Self::InvalidCompanionSelection
            | Self::SpeechModelNotInstalled
            | Self::ForegroundActivityBusy
            | Self::DictationUnavailable
            | Self::InvalidDictationBinding
            | Self::InstalledModelNotFound
            | Self::InstalledModelInvalid
            | Self::VisionProjectorNotInstalled
            | Self::InvalidInferenceLimits
            | Self::OllamaServiceUnavailable
            | Self::StorageUnavailable
            | Self::ProjectNotFound
            | Self::InvalidProjectName
            | Self::InvalidWorkspaceSelection
            | Self::WorkspaceGrantNotFound
            | Self::WorkspaceOperationsUnavailable
            | Self::AssetOperationsUnavailable
            | Self::AssetNotFound
            | Self::ConversationNotFound
            | Self::UnsupportedConversationContent => None,
            Self::WorkspaceAuthorization(error) => Some(error),
            Self::DiagnosticExport(error) => Some(error),
            Self::TransactionAudit(error) => Some(error),
        }
    }
}

impl From<std::io::Error> for AppCoreError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<StorageError> for AppCoreError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<FilesystemToolError> for AppCoreError {
    fn from(error: FilesystemToolError) -> Self {
        Self::Filesystem(error)
    }
}

impl From<ProjectComputationInputsError> for AppCoreError {
    fn from(error: ProjectComputationInputsError) -> Self {
        Self::ProjectComputation(error)
    }
}

impl From<FilesystemWriteError> for AppCoreError {
    fn from(error: FilesystemWriteError) -> Self {
        Self::FilesystemWrite(error)
    }
}

impl From<FilesystemCreateDirectoryError> for AppCoreError {
    fn from(error: FilesystemCreateDirectoryError) -> Self {
        Self::FilesystemCreateDirectory(error)
    }
}

impl From<CatalogueError> for AppCoreError {
    fn from(error: CatalogueError) -> Self {
        Self::ModelCatalogue(error)
    }
}

impl From<DownloadError> for AppCoreError {
    fn from(error: DownloadError) -> Self {
        Self::ModelDownload(error)
    }
}

impl From<OllamaError> for AppCoreError {
    fn from(error: OllamaError) -> Self {
        Self::Ollama(error)
    }
}

impl From<OllamaSidecarError> for AppCoreError {
    fn from(error: OllamaSidecarError) -> Self {
        Self::OllamaSidecar(error)
    }
}

impl From<ImportError> for AppCoreError {
    fn from(error: ImportError) -> Self {
        Self::ModelImport(error)
    }
}

impl From<TokenizerError> for AppCoreError {
    fn from(error: TokenizerError) -> Self {
        Self::Tokenizer(error)
    }
}

impl From<InferenceError> for AppCoreError {
    fn from(error: InferenceError) -> Self {
        Self::Inference(error)
    }
}

impl From<AssetStoreError> for AppCoreError {
    fn from(error: AssetStoreError) -> Self {
        Self::AssetStore(error)
    }
}

impl From<web_retrieval::RemoteImageHandoffError> for AppCoreError {
    fn from(error: web_retrieval::RemoteImageHandoffError) -> Self {
        Self::RemoteImageHandoff(error)
    }
}

impl From<PromptAdapterError> for AppCoreError {
    fn from(error: PromptAdapterError) -> Self {
        Self::ModelPrompt(error)
    }
}

impl From<AgentSystemInstructionError> for AppCoreError {
    fn from(error: AgentSystemInstructionError) -> Self {
        Self::AgentSystemInstructions(error)
    }
}

impl From<WorkingContextError> for AppCoreError {
    fn from(error: WorkingContextError) -> Self {
        Self::WorkingContext(error)
    }
}

impl From<ContextAdmissionError> for AppCoreError {
    fn from(error: ContextAdmissionError) -> Self {
        Self::ContextAdmission(error)
    }
}

impl From<DiagnosticExportError> for AppCoreError {
    fn from(error: DiagnosticExportError) -> Self {
        Self::DiagnosticExport(error)
    }
}

impl From<TransactionAuditError> for AppCoreError {
    fn from(error: TransactionAuditError) -> Self {
        Self::TransactionAudit(error)
    }
}

impl From<model_runtime::asr::SpeechModelError> for AppCoreError {
    fn from(error: model_runtime::asr::SpeechModelError) -> Self {
        Self::SpeechModel(error)
    }
}

impl From<model_runtime::ExternalModelError> for AppCoreError {
    fn from(error: model_runtime::ExternalModelError) -> Self {
        Self::ExternalModel(error)
    }
}
