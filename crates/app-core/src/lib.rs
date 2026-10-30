//! Composition root for Zeta application use cases.

mod web;

pub use web::{
    AuthorizedSearchRequest, AuthorizedWebRequest, PreparedSearchToolResult, PreparedWebToolResult,
    RemoteAssetProvenance, RemoteAssetReceipt, StartedSearchRequest, StartedWebRequest,
    WEB_RETRIEVE_TOOL, WEB_SEARCH_TOOL, WebAuthorizationError, WebDiagnosticOutcome,
    WebSearchAuthorizationError, WebSearchError, WebToolResultError, authorize_web_request,
    authorize_web_search_request, prepare_web_search_tool_result, prepare_web_tool_result,
    project_web_extraction, validate_web_search_tool_request, validate_web_tool_request,
    web_permission_scope, web_retrieval_reuse_digest, web_search_permission_scope,
    web_search_tool_registry, web_tool_registry,
};

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    error::Error,
    fmt, fs,
    path::Path,
    sync::{Arc, Mutex, atomic::AtomicBool},
};

#[cfg(test)]
use agent_core::AGENT_SYSTEM_POLICY_VERSION;
use agent_core::{
    AgentSystemInstructionError, AgentSystemInstructionIdentity, ContextAdmissionController,
    ContextAdmissionError, ContextAdmissionPolicy, ContextTokenEstimate, ExecutionBudgets,
    ExecutionProfile, ExecutionProfileDecision, ExecutionWorkKind, RoundAllowance,
    WorkingContextError, build_agent_system_instructions,
};
use app_storage::{
    ApplicationStore, AssetReference, AuditEvent, AuditEventKind, AuditFilesystemTool,
    AuditOutcome, AuditPermissionDecision, ModelCompanionMetadata, ModelMetadata, Project,
    SettingKey, SettingValue, StorageError, WorkspaceGrant,
};
use asset_store::{AssetFileStore, AssetStoreError};
pub use audit_log::{
    ApprovalAuditDecision, ApprovalFinalizationAuditOutcome, GenerationFailureCategory,
    OwnerAuthorizationAuditOutcome, TransactionAuditDeleteReceipt, TransactionAuditError,
    TransactionAuditEvent, TransactionAuditEventKind, TransactionAuditExportPlan,
    TransactionAuditExportPreview, TransactionAuditExportReceipt, TransactionAuditFailureCode,
    TransactionAuditOrigin, TransactionAuditResult, TransactionAuditState,
    TransactionAuditTransactionType, TransactionAuditWarningCode, X402AuditStage,
};
use audit_log::{
    Component, DiagnosticEvent, DiagnosticExportError, DiagnosticExportPlan,
    DiagnosticExportReceipt, DiagnosticLogger, DiagnosticOutcome, LogWriteOutcome, Operation,
    TransactionAuditLogger,
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use filesystem_tools::{
    FilesystemCreateDirectoryError, FilesystemCreateDirectoryResult, FilesystemToolError,
    FilesystemUndoChange, FilesystemWriteError, WorkspaceAuthorizationError, WorkspaceOperation,
};
use ipc_types::{
    AssetId, Conversation, ConversationId, ConversationSummary, FilesystemAppliedChange,
    FilesystemEntry, FilesystemEntryKind, FilesystemFindPathKind, FilesystemFindPathQuery,
    FilesystemFindPathQueryResult, FilesystemFindPathsResult, FilesystemListResult,
    FilesystemMetadata, FilesystemReadTextResult, FilesystemScanMatch, FilesystemScanResult,
    FilesystemSearchMatch, FilesystemSearchResult, FilesystemTextReplacement,
    FilesystemUndoFileResult, FilesystemUndoResult, FilesystemUndoStatus, FilesystemWriteResult,
    FilesystemWriteTextChange, IPC_SCHEMA_VERSION, JsonValue, MAX_PROJECT_NAME_BYTES, Message,
    MessageId, MessagePart, MessageRole, ModelSelection, ProjectId, ProjectSummary,
    ProjectWorkspaceRole, ProjectWorkspaceSummary, TaskActivity, ToolName, ToolOutcome,
    ToolResultPart, WorkspaceGrantId, WorkspaceGrantSummary, WorkspacePermissionLevel,
};
use model_runtime::{
    CatalogueError, CompanionArtifact, CompanionKind, CompanionLoadConfig, DownloadCancellation,
    DownloadError, DownloadProgress, ExactTokenizer, GLIMMER_IMAGE_TOKENS, GenerationBinding,
    GenerationEvent, ImportError, ImportProgress, ImportedModelArtifact, InferenceBackend,
    InferenceError, InferenceRuntimeStatus, LocalInferenceRuntime, MAX_GENERATION_IMAGES,
    ModelAdapterKind, ModelArtifact, ModelCatalogue, ModelDownloadManager, ModelImportManager,
    ModelKey, ModelLoadConfig, OllamaError, OllamaProvider, OllamaProviderConfig, OllamaSidecar,
    OllamaSidecarConfig, OllamaSidecarError, PromptAdapterError, PromptContent, PromptMessage,
    PromptRenderRequest, PromptToolCall, PromptToolDefinition, QwenAdapter, QwenStreamDecoder,
    ReasoningStrength, TokenizerError, bundled_catalogue, render_model_prompt,
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const MAX_PRODUCT_CONTEXT_TOKENS: u32 = 262_144;
const MAX_PRODUCT_OUTPUT_TOKENS: u32 = 8_192;
const DEFAULT_CONVERSATION_TITLE: &str = "New conversation";
#[cfg(test)]
const RECOMMENDED_MODEL_ID: &str = "qwen/qwen3.8-27b";
const GLIMMER_OLLAMA_TAG: &str = "zeta/muse-glimmer-30b:4cc57c0f5104-v2";

mod execution_context;
use execution_context::WorkingPromptAdapterContext;
pub use execution_context::*;

mod evm_transactions;
pub use app_storage::{StoredEvmDispatchPhase, StoredEvmTransaction, StoredEvmTransactionState};

mod wallet_metadata;
pub use app_storage::PublicWalletMetadata;

mod transaction_approvals;
pub use transaction_approvals::DurableApprovalConsumption;

mod transaction_audit;

mod chat_generation;
#[cfg(test)]
use chat_generation::{
    execution_round_budget, focused_phase_messages, product_output_tokens, prompt_message,
    reasoning_strength,
};
#[cfg(test)]
use conversations::derive_conversation_title;
#[cfg(test)]
use models::{
    complete_bundle_companions, resolve_new_conversation_selection, validate_inference_limits,
};
mod computation;
mod conversations;
mod diagnostics;
mod dictation;
mod models;
pub use models::ExternalModelAccess;
mod shutdown;
use models::ollama_service::{OllamaService, OllamaShutdown};
mod working_context;
mod workspace;

pub use computation::{
    ComputationResultError, PROJECT_COMPUTATION_TOOL, PURE_COMPUTATION_TOOL,
    PreparedComputationResult, ProjectComputationInputBinding, ProjectComputationInputReference,
    ProjectComputationInputSnapshot, ProjectComputationInputsError, computation_reuse_digest,
    computation_runner_error_status, computation_terminal_status, computation_tool_registry,
    prepare_computation_result,
};
pub use dictation::{ChatActivityLease, DictationCancelReason};

pub use chat_generation::{
    ChatPromptContinuation, RichInputIdentity, RichInputPresentationPolicy, ToolScheduleLimits,
};

/// Maximum encoded bytes accepted by the managed image-import boundary.
pub const MAX_IMAGE_IMPORT_BYTES: usize = asset_store::MAX_ENCODED_BYTES;

/// One artifact-scoped snapshot within an aggregate catalogue installation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogueInstallProgress {
    artifact_id: String,
    companion_kind: Option<CompanionKind>,
    artifact: DownloadProgress,
    aggregate_downloaded_bytes: u64,
    aggregate_total_bytes: u64,
}

impl CatalogueInstallProgress {
    /// Returns the model-local primary or companion artifact identifier.
    #[must_use]
    pub fn artifact_id(&self) -> &str {
        &self.artifact_id
    }

    /// Returns the companion role, or `None` for the primary model.
    #[must_use]
    pub const fn companion_kind(&self) -> Option<CompanionKind> {
        self.companion_kind
    }

    /// Returns the independently verified artifact progress snapshot.
    #[must_use]
    pub const fn artifact(&self) -> DownloadProgress {
        self.artifact
    }

    /// Returns bytes available across the complete selected profile.
    #[must_use]
    pub const fn aggregate_downloaded_bytes(&self) -> u64 {
        self.aggregate_downloaded_bytes
    }

    /// Returns total bytes across the complete selected profile.
    #[must_use]
    pub const fn aggregate_total_bytes(&self) -> u64 {
        self.aggregate_total_bytes
    }

    /// Returns bytes remaining across the complete selected profile.
    #[must_use]
    pub const fn aggregate_remaining_bytes(&self) -> u64 {
        self.aggregate_total_bytes
            .saturating_sub(self.aggregate_downloaded_bytes)
    }
}

/// Successful installation state for a selected catalogue profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogueInstallReceipt {
    installed_bytes: u64,
    artifact_ids: Vec<String>,
}

impl CatalogueInstallReceipt {
    /// Returns aggregate installed bytes for the primary and selected companions.
    #[must_use]
    pub const fn installed_bytes(&self) -> u64 {
        self.installed_bytes
    }

    /// Returns installed artifact identifiers in stable profile order.
    #[must_use]
    pub fn artifact_ids(&self) -> &[String] {
        &self.artifact_ids
    }
}

/// Long-lived Rust application services managed by the native shell.
pub struct AppCore {
    shutdown_started: AtomicBool,
    speech_models: Arc<dictation::SpeechModels>,
    foreground_activity: Arc<dictation::ForegroundActivityGate>,
    dictation_sessions: Arc<dictation::DictationSessions>,
    model_catalogue: ModelCatalogue,
    model_downloads: ModelDownloadManager,
    model_imports: ModelImportManager,
    model_runtime: LocalInferenceRuntime,
    exact_tokenizer: ExactTokenizer,
    ollama_models_directory: Option<std::path::PathBuf>,
    ollama_service: Mutex<OllamaService>,
    ollama_shutdown: OllamaShutdown,
    ollama_generations: Mutex<HashMap<String, DownloadCancellation>>,
    storage: Mutex<ApplicationStore>,
    workspace_operations: workspace::WorkspaceOperationGate,
    private_data: filesystem_tools::PrivateDirectory,
    asset_files: AssetFileStore,
    asset_operations: Mutex<()>,
    diagnostics: DiagnosticLogger,
    transaction_audit: TransactionAuditLogger,
    #[cfg(debug_assertions)]
    development_tool_trace: diagnostics::DevelopmentToolTrace,
    // Drop file scopes after both inference providers and their process owners.
    external_model_access: Arc<dyn ExternalModelAccess>,
}

/// Content-free boundary within post-execution computation handling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComputationDiagnosticStage {
    /// Validate and sanitize the runner's terminal result and provenance.
    PrepareResult,
    /// Persist the compact result before exposing it to the model.
    PersistResult,
    /// Return the terminal envelope through the generic agent state machine.
    ReturnAgentResult,
    /// Convert the trusted envelope into the closed desktop IPC result.
    ProjectIpcResult,
}

impl ComputationDiagnosticStage {
    const fn diagnostic_coordinates(self) -> (Component, Operation) {
        match self {
            Self::PrepareResult => (Component::Computation, Operation::Validate),
            Self::PersistResult => (Component::Storage, Operation::Persist),
            Self::ReturnAgentResult => (Component::Agent, Operation::ExecuteTool),
            Self::ProjectIpcResult => (Component::Application, Operation::Validate),
        }
    }

    /// Returns the bounded stage name used only by the local development trace.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::PrepareResult => "prepare_result",
            Self::PersistResult => "persist_result",
            Self::ReturnAgentResult => "return_agent_result",
            Self::ProjectIpcResult => "project_ipc_result",
        }
    }
}

impl AppCore {
    /// Creates the private application-data directory and opens local persistence.
    ///
    /// # Errors
    ///
    /// Returns an error when private files cannot be prepared or the application
    /// database cannot be opened, migrated and validated safely.
    #[cfg(any(test, debug_assertions))]
    pub fn initialize(data_directory: impl AsRef<Path>) -> Result<Self, AppCoreError> {
        let tokenizer = data_directory.as_ref().join("zeta-tokenizer-test");
        Self::initialize_with_ollama(
            data_directory.as_ref(),
            OllamaService::external(OllamaProvider::new(OllamaProviderConfig::default())?),
            &tokenizer,
            None,
        )
    }

    /// Starts the pinned bundled Ollama runtime with a Zeta-private model store.
    ///
    /// # Errors
    ///
    /// Returns an error when the private application data, bundled runtime,
    /// exact version health check or ordinary application services fail safely.
    pub fn initialize_with_bundled_ollama(
        data_directory: impl AsRef<Path>,
        ollama_executable: impl AsRef<Path>,
        tokenizer_executable: impl AsRef<Path>,
    ) -> Result<Self, AppCoreError> {
        Self::initialize_with_bundled_ollama_guardian(
            data_directory,
            ollama_executable,
            tokenizer_executable,
            None,
        )
    }

    /// Starts bundled Ollama under a native parent-death guardian.
    ///
    /// # Errors
    ///
    /// Returns an error when the private application data, bundled runtime,
    /// guardian, exact version health check or ordinary services fail safely.
    pub fn initialize_with_guarded_bundled_ollama(
        data_directory: impl AsRef<Path>,
        ollama_executable: impl AsRef<Path>,
        tokenizer_executable: impl AsRef<Path>,
        guardian_executable: impl AsRef<Path>,
    ) -> Result<Self, AppCoreError> {
        Self::initialize_with_bundled_ollama_guardian(
            data_directory,
            ollama_executable,
            tokenizer_executable,
            Some(guardian_executable.as_ref()),
        )
    }

    fn initialize_with_bundled_ollama_guardian(
        data_directory: impl AsRef<Path>,
        ollama_executable: impl AsRef<Path>,
        tokenizer_executable: impl AsRef<Path>,
        guardian_executable: Option<&Path>,
    ) -> Result<Self, AppCoreError> {
        let data_directory = data_directory.as_ref();
        fs::create_dir_all(data_directory)?;
        restrict_directory_permissions(data_directory)?;
        let sidecar = OllamaSidecar::new();
        let mut sidecar_config = models::tuning::configure_sidecar(OllamaSidecarConfig::new(
            ollama_executable.as_ref(),
            data_directory.join("ollama-models"),
        ))?;
        if let Some(guardian_executable) = guardian_executable {
            sidecar_config = sidecar_config.with_guardian_executable(guardian_executable);
            #[cfg(debug_assertions)]
            {
                sidecar_config = sidecar_config
                    .with_cache_telemetry_directory(data_directory.join("dev-diagnostics"));
            }
        }
        let endpoint = sidecar.start(&sidecar_config)?;
        let provider = OllamaProvider::new(models::tuning::configure(
            OllamaProviderConfig::loopback(endpoint.address().into())?,
            true,
        )?)?;
        Self::initialize_with_ollama(
            data_directory,
            OllamaService::bundled(provider, sidecar, sidecar_config),
            tokenizer_executable.as_ref(),
            Some(data_directory.join("ollama-models")),
        )
    }

    /// Uses an explicit user-supplied development Ollama endpoint.
    ///
    /// This entry point does not exist in release builds, preventing production
    /// from silently discovering or sharing a separately installed server.
    ///
    /// # Errors
    ///
    /// Returns an error when the explicit endpoint is unavailable or ordinary
    /// private application services cannot be initialized safely.
    #[cfg(debug_assertions)]
    pub fn initialize_with_development_ollama(
        data_directory: impl AsRef<Path>,
        config: OllamaProviderConfig,
        tokenizer_executable: impl AsRef<Path>,
        ollama_models_directory: impl AsRef<Path>,
    ) -> Result<Self, AppCoreError> {
        Self::initialize_with_ollama(
            data_directory.as_ref(),
            OllamaService::external(OllamaProvider::new(models::tuning::configure(
                config, false,
            )?)?),
            tokenizer_executable.as_ref(),
            Some(ollama_models_directory.as_ref().to_owned()),
        )
    }

    fn initialize_with_ollama(
        data_directory: &Path,
        ollama_service: OllamaService,
        tokenizer_executable: &Path,
        ollama_models_directory: Option<std::path::PathBuf>,
    ) -> Result<Self, AppCoreError> {
        let model_catalogue = bundled_catalogue()?;
        fs::create_dir_all(data_directory)?;
        restrict_directory_permissions(data_directory)?;
        let database_path = data_directory.join("zeta.sqlite3");
        prepare_private_database_file(&database_path)?;
        let mut storage = ApplicationStore::open(database_path)?;
        let storage_schema_version = storage.schema_version()?;
        let diagnostics_enabled = match storage.setting(SettingKey::DiagnosticLogging)? {
            Some(SettingValue::DiagnosticLogging(enabled)) => enabled,
            Some(_) => {
                return Err(StorageError::InvalidRecord(
                    "diagnostic logging setting has an incompatible type",
                )
                .into());
            }
            None => true,
        };
        let asset_files = AssetFileStore::initialize(data_directory.join("assets"))?;
        let model_downloads = ModelDownloadManager::initialize(data_directory.join("models"))?;
        let model_imports = ModelImportManager::initialize(data_directory.join("models"))?;
        let reaped_assets = storage.reap_unreferenced_assets()?;
        asset_files.remove_assets(&reaped_assets)?;
        asset_files.reconcile(&storage.assets()?)?;
        let ollama_shutdown = ollama_service.shutdown_handle();
        let app = Self {
            shutdown_started: AtomicBool::new(false),
            speech_models: Arc::new(dictation::SpeechModels::initialize(
                &data_directory.join("speech-models"),
            )?),
            foreground_activity: Arc::new(dictation::ForegroundActivityGate::new()),
            dictation_sessions: Arc::new(dictation::DictationSessions::new()),
            model_catalogue,
            model_downloads,
            model_imports,
            model_runtime: LocalInferenceRuntime::new()?,
            external_model_access: Arc::new(models::DirectFileAccess),
            exact_tokenizer: ExactTokenizer::new(tokenizer_executable),
            ollama_models_directory,
            ollama_service: Mutex::new(ollama_service),
            ollama_shutdown,
            ollama_generations: Mutex::new(HashMap::new()),
            storage: Mutex::new(storage),
            workspace_operations: workspace::WorkspaceOperationGate::new(),
            private_data: filesystem_tools::PrivateDirectory::open(data_directory)
                .map_err(FilesystemToolError::from)?,
            asset_files,
            asset_operations: Mutex::new(()),
            diagnostics: DiagnosticLogger::new(
                data_directory.join("diagnostics"),
                diagnostics_enabled,
            ),
            transaction_audit: TransactionAuditLogger::new(
                data_directory.join("transaction-audit"),
            ),
            #[cfg(debug_assertions)]
            development_tool_trace: diagnostics::DevelopmentToolTrace::new(
                data_directory.join("dev-diagnostics"),
            ),
        };
        let _ = app.record_diagnostic(DiagnosticEvent::ApplicationStarted {
            ipc_schema_version: IPC_SCHEMA_VERSION,
            storage_schema_version,
        });
        Ok(app)
    }

    /// Returns the Rust-owned local inference supervisor.
    #[must_use]
    pub const fn model_runtime(&self) -> &LocalInferenceRuntime {
        &self.model_runtime
    }

    /// Acquires exclusive foreground admission for one chat generation.
    ///
    /// The returned lease releases admission on every caller cleanup path.
    ///
    /// # Errors
    /// Rejects generation while chat generation or dictation is already active.
    pub fn begin_chat_activity(&self) -> Result<ChatActivityLease, AppCoreError> {
        self.foreground_activity.begin_chat()
    }
}

impl Drop for AppCore {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

mod error;
mod projection;
pub use error::AppCoreError;
use projection::{
    filesystem_find_paths_result, filesystem_list_result, filesystem_metadata,
    filesystem_read_text_result, filesystem_scan_result, filesystem_search_result,
    filesystem_undo_result, filesystem_write_result, project_summary, workspace_summary,
};

fn prepare_private_database_file(path: &Path) -> Result<(), std::io::Error> {
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    drop(options.open(path)?);
    #[cfg(unix)]
    fs::set_permissions(path, unix_permissions(0o600))?;
    Ok(())
}

fn restrict_directory_permissions(path: &Path) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    fs::set_permissions(path, unix_permissions(0o700))?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(unix)]
fn unix_permissions(mode: u32) -> fs::Permissions {
    use std::os::unix::fs::PermissionsExt;
    fs::Permissions::from_mode(mode)
}

#[cfg(test)]
mod tests;
