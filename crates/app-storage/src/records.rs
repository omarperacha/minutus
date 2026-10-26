//! Typed non-secret records stored in the application database.

use ipc_types::{
    AssetId, ProjectId, ProjectWorkspaceRole, WalletId, WorkspaceGrantId, WorkspacePermissionLevel,
};
use serde::{Deserialize, Serialize};

/// Known application setting names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingKey {
    /// User interface color preference.
    Theme,
    /// Currently selected local model.
    SelectedModel,
    /// Whether local diagnostic logging is enabled.
    DiagnosticLogging,
    /// User interface locale.
    Locale,
    /// Reduced-motion accessibility preference.
    ReduceMotion,
}

impl SettingKey {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Theme => "theme",
            Self::SelectedModel => "selected_model",
            Self::DiagnosticLogging => "diagnostic_logging",
            Self::Locale => "locale",
            Self::ReduceMotion => "reduce_motion",
        }
    }
}

/// User interface theme preference.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    /// Follow the operating-system preference.
    System,
    /// Use the light theme.
    Light,
    /// Use the dark theme.
    Dark,
}

/// Closed set of non-secret application setting values.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum SettingValue {
    /// User interface theme.
    Theme(Theme),
    /// Opaque identifier of the selected model, or no selection.
    SelectedModel(Option<String>),
    /// Local diagnostic logging preference.
    DiagnosticLogging(bool),
    /// User interface locale identifier.
    Locale(String),
    /// Reduced-motion accessibility preference.
    ReduceMotion(bool),
}

impl SettingValue {
    pub(crate) const fn key(&self) -> SettingKey {
        match self {
            Self::Theme(_) => SettingKey::Theme,
            Self::SelectedModel(_) => SettingKey::SelectedModel,
            Self::DiagnosticLogging(_) => SettingKey::DiagnosticLogging,
            Self::Locale(_) => SettingKey::Locale,
            Self::ReduceMotion(_) => SettingKey::ReduceMotion,
        }
    }
}

/// Persisted access grant for one user-selected workspace root.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceGrant {
    /// Opaque grant identifier.
    pub id: WorkspaceGrantId,
    /// User-visible folder name.
    pub display_name: String,
    /// Canonical path used only by trusted Rust code.
    pub canonical_root: String,
    /// Opaque platform security-scoped bookmark data.
    pub bookmark_data: Vec<u8>,
    /// Persisted capability level enforced by the Rust filesystem broker.
    pub permission_level: WorkspacePermissionLevel,
    /// RFC 3339 creation timestamp.
    pub created_at: String,
    /// RFC 3339 revocation timestamp, when revoked.
    pub revoked_at: Option<String>,
}

/// Persisted user-visible project that owns chats and folder settings.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Project {
    /// Opaque project identifier.
    pub id: ProjectId,
    /// User-visible project name.
    pub name: String,
    /// RFC 3339 creation timestamp.
    pub created_at: String,
    /// RFC 3339 timestamp of the most recent project change.
    pub updated_at: String,
}

/// One granted folder attached to a project.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectWorkspace {
    /// Owning project.
    pub project_id: ProjectId,
    /// Underlying durable folder grant.
    pub grant: WorkspaceGrant,
    /// Primary or additional role within the project.
    pub role: ProjectWorkspaceRole,
    /// RFC 3339 attachment timestamp.
    pub added_at: String,
}

/// Metadata for one installed local model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelMetadata {
    /// Opaque model identifier.
    pub id: String,
    /// Human-readable model name.
    pub display_name: String,
    /// On-disk model format, such as `gguf`.
    pub format: String,
    /// Rust-owned local artifact location.
    pub local_path: String,
    /// Lowercase content digest of the artifact.
    pub content_digest: String,
    /// Artifact length in bytes.
    pub byte_size: u64,
    /// RFC 3339 import timestamp.
    pub imported_at: String,
}

/// Metadata for one verified companion belonging to an installed model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelCompanionMetadata {
    /// Installed primary model identity owning this companion.
    pub model_id: String,
    /// Model-local companion artifact identifier.
    pub artifact_id: String,
    /// Stable companion role such as `visionProjector` or `draftModel`.
    pub kind: String,
    /// Whether all profiles for the model require this artifact.
    pub required: bool,
    /// Rust-owned local artifact location.
    pub local_path: String,
    /// Lowercase SHA-256 digest of the artifact.
    pub content_digest: String,
    /// Artifact length in bytes.
    pub byte_size: u64,
    /// Exact model/companion compatibility group.
    pub compatibility_version: String,
    /// Additional recommended unified memory in bytes.
    pub memory_requirement_bytes: u64,
    /// RFC 3339 installation timestamp.
    pub installed_at: String,
}

/// Metadata for one Rust-owned file belonging to a managed asset.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssetFileReference {
    /// Rust-owned storage key, never a model-supplied path.
    pub storage_key: String,
    /// Validated media type.
    pub media_type: String,
    /// Lowercase SHA-256 digest of the stored bytes.
    pub content_digest: String,
    /// Stored length in bytes.
    pub byte_size: u64,
    /// Decoded pixel width after orientation correction.
    pub width: u32,
    /// Decoded pixel height after orientation correction.
    pub height: u32,
}

/// Metadata and stable storage references for one imported asset.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssetReference {
    /// Stable opaque asset identifier used by conversation records.
    pub id: AssetId,
    /// Exact locally imported bytes, kept separately from inference input.
    pub original: AssetFileReference,
    /// Validated, resized and metadata-free inference copy.
    pub derivative: AssetFileReference,
    /// RFC 3339 import timestamp.
    pub imported_at: String,
}

/// Non-secret public wallet metadata stored outside the wallet vault.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicWalletMetadata {
    /// Opaque wallet identifier.
    pub id: WalletId,
    /// Public account address.
    pub address: String,
    /// CAIP-2 network identifier.
    pub network: String,
    /// Whether the separate wallet vault is currently locked.
    pub locked: bool,
    /// RFC 3339 update timestamp.
    pub updated_at: String,
}

/// Persisted public lifecycle for a submitted EVM transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoredEvmTransactionState {
    /// Submitted without a receipt.
    Pending,
    /// Included with successful receipt status.
    Confirmed,
    /// Included with failed receipt status.
    Failed,
    /// Absent after the sender nonce advanced.
    DroppedOrReplaced,
}

/// Non-secret metadata sufficient to resume EVM receipt monitoring.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredEvmTransaction {
    /// Canonical transaction hash.
    pub transaction_hash: String,
    /// Trusted CAIP-2 network identity.
    pub network: String,
    /// Public sending address.
    pub sender: String,
    /// Submitted nonce as an exact decimal integer.
    pub nonce: String,
    /// Digest of the approved prepared candidate.
    pub candidate_digest: String,
    /// Latest chain-derived lifecycle.
    pub state: StoredEvmTransactionState,
    /// Inclusion block number for receipt-backed terminal states.
    pub block_number: Option<String>,
    /// Inclusion block hash for receipt-backed terminal states.
    pub block_hash: Option<String>,
    /// RFC 3339 node-acceptance timestamp.
    pub submitted_at: String,
    /// RFC 3339 latest successful observation timestamp.
    pub updated_at: String,
}

/// Exact public metadata for one terminal approval use decision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredApprovalFinalization {
    /// Cryptographically random, single-use approval identifier.
    pub approval_id: String,
    /// Digest binding the identifier and every approved fact.
    pub commitment_digest: String,
    /// Public digest presented to the EVM signing algorithm.
    pub signing_digest: String,
    /// Exact chain ID encoded as an unsigned decimal integer.
    pub chain_id: String,
    /// Exact public signing account.
    pub account: String,
    /// Exact trusted host session identifier.
    pub session_id: String,
    /// Absolute approval expiry encoded as an unsigned decimal integer.
    pub expires_at_unix: String,
    /// Closed transaction type code.
    pub transaction_type: String,
    /// Approval policy revision encoded as an unsigned decimal integer.
    pub policy_revision: String,
    /// Terminal disposition: consumed or invalidated.
    pub disposition: String,
    /// Bounded invalidation reason, absent for consumption.
    pub invalidation_reason: Option<String>,
    /// Trusted finalization time encoded as an unsigned decimal integer.
    pub finalized_at_unix: String,
}

/// Result of atomically finalizing an approval identifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoredApprovalFinalizationOutcome {
    /// The requested row was inserted by this call.
    Finalized,
    /// The exact commitment was already consumed.
    AlreadyConsumed,
    /// The exact commitment was already invalidated.
    Invalidated,
    /// The identifier already belongs to different binding metadata.
    IdentifierCollision,
}

/// Allow-listed non-secret audit event categories.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuditEventKind {
    /// Application persistence was initialized.
    ApplicationOpened,
    /// A project was created.
    ProjectCreated,
    /// A workspace capability was granted.
    WorkspaceGranted,
    /// A workspace capability level was changed.
    WorkspacePermissionChanged,
    /// A workspace capability was revoked.
    WorkspaceRevoked,
    /// A permission-gated filesystem tool was invoked.
    FilesystemTool,
    /// A local model was registered.
    ModelRegistered,
    /// A conversation was deleted.
    ConversationDeleted,
}

impl AuditEventKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::ApplicationOpened => "application_opened",
            Self::ProjectCreated => "project_created",
            Self::WorkspaceGranted => "workspace_granted",
            Self::WorkspacePermissionChanged => "workspace_permission_changed",
            Self::WorkspaceRevoked => "workspace_revoked",
            Self::FilesystemTool => "filesystem_tool",
            Self::ModelRegistered => "model_registered",
            Self::ConversationDeleted => "conversation_deleted",
        }
    }
}

/// Closed set of filesystem tools permitted in durable audit metadata.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuditFilesystemTool {
    /// Lists one directory.
    List,
    /// Finds safe entries recursively by filename filters.
    FindPaths,
    /// Reads metadata for one entry.
    Stat,
    /// Reads bounded UTF-8 text.
    ReadText,
    /// Imports one bounded workspace image for inspection.
    InspectImage,
    /// Searches bounded UTF-8 text.
    Search,
    /// Scans aggregate file metadata.
    Scan,
    /// Replaces complete file contents.
    WriteText,
    /// Creates exactly one directory beneath an existing parent.
    CreateDirectory,
    /// Applies exact text substitutions.
    ReplaceText,
    /// Reverts an applied text change.
    UndoText,
    /// Checks whether a text change can be reverted.
    UndoAvailable,
}

impl AuditFilesystemTool {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::List => "filesystem.list",
            Self::FindPaths => "filesystem.find_paths",
            Self::Stat => "filesystem.stat",
            Self::ReadText => "filesystem.read_text",
            Self::InspectImage => "filesystem.inspect_image",
            Self::Search => "filesystem.search",
            Self::Scan => "filesystem.scan",
            Self::WriteText => "filesystem.write_text",
            Self::CreateDirectory => "filesystem.create_directory",
            Self::ReplaceText => "filesystem.replace_text",
            Self::UndoText => "filesystem.undo_text",
            Self::UndoAvailable => "filesystem.undo_available",
        }
    }
}

/// Permission decision made before an audited operation took effect.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuditPermissionDecision {
    /// The active grant permitted the operation.
    Allowed,
    /// The operation was denied before an effect.
    Denied,
    /// The event does not represent a permission-gated filesystem operation.
    NotApplicable,
}

impl AuditPermissionDecision {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Allowed => "allowed",
            Self::Denied => "denied",
            Self::NotApplicable => "not_applicable",
        }
    }
}

/// Allow-listed audit outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuditOutcome {
    /// Operation completed.
    Succeeded,
    /// Operation was denied before an effect.
    Denied,
    /// Operation failed without recording sensitive diagnostics.
    Failed,
}

impl AuditOutcome {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Denied => "denied",
            Self::Failed => "failed",
        }
    }
}

/// One allow-listed non-secret audit event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditEvent {
    /// Opaque event identifier.
    pub id: String,
    /// Stable allow-listed event category.
    pub kind: AuditEventKind,
    /// Stable allow-listed result.
    pub outcome: AuditOutcome,
    /// Optional opaque identifier of the affected record.
    pub subject_id: Option<String>,
    /// Shared opaque identifier for events belonging to one operation.
    pub operation_id: Option<String>,
    /// Allow-listed filesystem tool, when applicable.
    pub tool: Option<AuditFilesystemTool>,
    /// Stable grant identifier, never a root path or bookmark.
    pub workspace_grant_id: Option<WorkspaceGrantId>,
    /// Validated workspace-relative path, never an absolute path.
    pub relative_path: Option<String>,
    /// Broker permission decision made before the operation.
    pub permission_decision: AuditPermissionDecision,
    /// Bounded result cardinality, when useful.
    pub result_count: Option<u32>,
    /// Whether a bounded result omitted additional matches or entries.
    pub truncated: Option<bool>,
    /// RFC 3339 occurrence timestamp.
    pub occurred_at: String,
}
