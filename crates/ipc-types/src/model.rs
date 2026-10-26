//! Frontend-safe model catalogue and download view models.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Exact immutable catalogue release selected for a default or conversation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelSelection {
    /// Publisher-qualified model identity.
    pub model_id: String,
    /// Immutable publisher revision.
    pub model_version: String,
}

/// Persisted application-level model preference.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelPreferences {
    /// Model assigned to newly-created chats, when one is configured.
    pub default_model: Option<ModelSelection>,
}

/// Request to change the default assigned only to future chats.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetDefaultModelRequest {
    /// Completely installed compatible release.
    pub selection: ModelSelection,
}

/// Request to explicitly rebind one persisted chat.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetConversationModelRequest {
    /// Conversation whose derived model state will be invalidated.
    pub conversation_id: crate::ConversationId,
    /// Completely installed compatible release.
    pub selection: ModelSelection,
}

/// Frontend-safe companion artifact role.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum ModelCompanionKind {
    /// Perception projector used by a multimodal inference profile.
    VisionProjector,
    /// Speculative-decoding draft model.
    DraftModel,
}

/// Downloadable companion exposed from the validated Rust catalogue.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogueCompanion {
    /// Model-local artifact identifier.
    pub artifact_id: String,
    /// Runtime role of the artifact.
    pub kind: ModelCompanionKind,
    /// Whether setup requires this module; loading still follows the active request.
    pub required: bool,
    /// Expected transferred and installed size.
    #[ts(type = "number")]
    pub download_bytes: u64,
    /// Additional recommended unified memory.
    #[ts(type = "number")]
    pub memory_requirement_bytes: u64,
    /// Whether matching verified bytes and metadata are installed.
    pub installed: bool,
}

/// Registration is durable; availability is evaluated separately from first installation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum ModelRegistrationState {
    /// No successful installation or external registration.
    NotInstalled,
    /// Verified modules exist, but required setup modules are still missing.
    Incomplete,
    /// Registered files are currently accessible and unchanged.
    Available,
    /// Previously registered files are missing or inaccessible.
    Unavailable,
    /// Registered files changed or no longer match their catalogue identity.
    VerificationFailed,
}

/// Source of a verified model module. No host paths cross IPC.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ModelModuleSource {
    /// Selected local file, used in place.
    Imported,
    /// Application-managed downloaded file.
    Downloaded,
}

/// Independently verifiable module and its persistent setup requirement.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogueModule {
    /// Catalogue-local artifact identity.
    pub artifact_id: String,
    /// Short role shown in setup and settings.
    pub display_name: String,
    /// Expected full filename shown in the native picker.
    pub file_name: String,
    /// Whether setup is blocked without this module.
    pub required: bool,
    /// Expected transfer size.
    #[ts(type = "number")]
    pub download_bytes: u64,
    /// Verified file availability, independently of sibling modules.
    pub registration_state: ModelRegistrationState,
    /// Origin of a previously verified file, when recorded.
    pub source: Option<ModelModuleSource>,
}

/// One downloadable model release exposed by the Rust-owned catalogue.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct CatalogueModel {
    /// Publisher-qualified model identity.
    pub id: String,
    /// Immutable publisher revision.
    pub version: String,
    /// Human-readable model name.
    pub display_name: String,
    /// Whether product policy recommends this model for new users.
    pub recommended: bool,
    /// Local provider used by this release.
    pub provider: String,
    /// Whether the provider can currently service control requests.
    pub provider_available: bool,
    /// Short, stable user-facing capability labels.
    pub capabilities: Vec<String>,
    /// Advertised maximum context window in tokens for this release.
    pub context_limit_tokens: u32,
    /// Expected download size in bytes.
    #[ts(type = "number")]
    pub download_bytes: u64,
    /// Expected transferred size for the primary and every declared companion.
    #[ts(type = "number")]
    pub bundle_download_bytes: u64,
    /// Recommended unified memory in bytes.
    #[ts(type = "number")]
    pub recommended_memory_bytes: u64,
    /// Publisher licence document.
    pub license_url: String,
    /// Publisher usage-policy document.
    pub usage_policy_url: String,
    /// Whether this release is present in Rust-managed installed storage.
    pub installed: bool,
    /// Durable registration and current file availability.
    pub registration_state: ModelRegistrationState,
    /// Whether the model references user-owned files rather than managed downloads.
    pub external_files: bool,
    /// Whether this profile supports selecting a complete local file bundle.
    pub local_import_supported: bool,
    /// Whether this build has both provider and adapter support for active chat.
    pub chat_selectable: bool,
    /// Required and optional companion artifacts in catalogue order.
    pub companions: Vec<CatalogueCompanion>,
    /// Independently selectable setup modules, primary first.
    pub modules: Vec<CatalogueModule>,
}

/// Exact catalogue release requested for installation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelDownloadRequest {
    /// Publisher-qualified model identity.
    pub model_id: String,
    /// Immutable publisher revision.
    pub model_version: String,
    /// Legacy companion hints retained for wire compatibility. The host
    /// validates these identifiers but installs every declared companion.
    pub companion_artifact_ids: Vec<String>,
}

/// Native phase of a catalogue model installation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum ModelDownloadPhase {
    /// Model bytes are being downloaded into resumable partial storage.
    Downloading,
    /// Complete bytes are being hashed against the catalogue SHA-256.
    Verifying,
    /// Verified bytes are being atomically published.
    Installing,
}

/// Byte progress emitted by the native model downloader.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelDownloadEvent {
    /// Publisher-qualified model identity.
    pub model_id: String,
    /// Immutable publisher revision.
    pub model_version: String,
    /// Artifact currently downloading, verifying or installing.
    pub artifact_id: String,
    /// Companion role, or `None` for the primary model artifact.
    pub companion_kind: Option<ModelCompanionKind>,
    /// Bytes retained so far.
    #[ts(type = "number")]
    pub downloaded_bytes: u64,
    /// Expected complete size.
    #[ts(type = "number")]
    pub total_bytes: u64,
    /// Bytes remaining before verification.
    #[ts(type = "number")]
    pub remaining_bytes: u64,
    /// Whether this attempt began from retained partial bytes.
    pub resumed: bool,
    /// Current native installation phase.
    pub phase: ModelDownloadPhase,
    /// Bytes available across the full selected profile.
    #[ts(type = "number")]
    pub aggregate_downloaded_bytes: u64,
    /// Total installed bytes across the full selected profile.
    #[ts(type = "number")]
    pub aggregate_total_bytes: u64,
    /// Bytes remaining across the full selected profile.
    #[ts(type = "number")]
    pub aggregate_remaining_bytes: u64,
}

/// Successful verified model installation result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelInstallReceipt {
    /// Publisher-qualified model identity.
    pub model_id: String,
    /// Immutable publisher revision.
    pub model_version: String,
    /// Verified installed artifact size.
    #[ts(type = "number")]
    pub installed_bytes: u64,
    /// Primary and companion artifact identifiers installed for this profile.
    pub artifact_ids: Vec<String>,
}

/// Byte progress emitted while a selected GGUF is copied to managed storage.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelImportEvent {
    /// Bytes copied so far.
    #[ts(type = "number")]
    pub copied_bytes: u64,
    /// Stable selected-file size.
    #[ts(type = "number")]
    pub total_bytes: u64,
    /// Bytes remaining before reinspection and publication.
    #[ts(type = "number")]
    pub remaining_bytes: u64,
}

/// Successful compatible GGUF import result, without filesystem paths.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelImportReceipt {
    /// Digest-derived local model identity.
    pub id: String,
    /// Model name read from validated GGUF metadata.
    pub display_name: String,
    /// Validated inference architecture.
    pub architecture: String,
    /// Model-declared maximum context length.
    #[ts(type = "number")]
    pub context_tokens: u32,
    /// Exact managed artifact size.
    #[ts(type = "number")]
    pub byte_size: u64,
    /// Lowercase SHA-256 of the managed copy.
    pub sha256: String,
}

/// Read-only verification progress for the complete external model bundle.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExternalModelProgress {
    /// Publisher-qualified model identity.
    pub model_id: String,
    /// Exact module whose selection or transfer is in progress.
    pub artifact_id: String,
    /// Download or verification phase for this module.
    pub phase: ModelDownloadPhase,
    /// Bytes transferred or read and hashed.
    #[ts(type = "number")]
    pub verified_bytes: u64,
    /// Total bundle bytes to verify.
    #[ts(type = "number")]
    pub total_bytes: u64,
}
