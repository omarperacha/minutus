//! Rust-owned, frontend-safe IPC schemas for Zeta.
//!
//! This crate contains data only. It owns no filesystem, network, model or
//! wallet capability and deliberately cannot represent wallet secret material.

mod activity;
mod attachment;
mod bindings;
mod conversation;
mod evm;
mod filesystem;
mod generation;
mod model;
mod permission;
mod project;
mod speech;
mod tool;
mod transaction;
mod validation;
mod wallet;
mod workspace;
mod x402;

pub use activity::{TaskActivity, TaskActivityKind, TaskActivityStatus};
pub use attachment::ImageAttachmentReceipt;
pub use bindings::typescript_bindings;
pub use conversation::{
    ChatEffort, ChatRequest, Conversation, ConversationSummary, Message, MessagePart, MessageRole,
    RenameConversationRequest,
};
pub use evm::{
    EvmApprovalDecision, EvmApprovalReviewOutcome, EvmAssetBalance, EvmAuthorizationDecodeRequest,
    EvmAuthorizationField, EvmAuthorizationKind, EvmAuthorizationPayloadKind,
    EvmAuthorizationStatus, EvmAuthorizationWarning, EvmAuthorizationWarningCode,
    EvmAuthorizationWarningSeverity, EvmBalanceFreshness, EvmBalanceRequest, EvmBalanceResponse,
    EvmBalanceSnapshot, EvmDecodedAuthorization, EvmDecodedTokenTransfer,
    EvmNativeTransferCandidate, EvmNativeTransferRequest, EvmNetwork, EvmRpcNetworkSetting,
    EvmRpcSettings, EvmSimulationComparison, EvmSimulationEffect, EvmSimulationEffectKind,
    EvmSimulationState, EvmTokenTransferCandidate, EvmTokenTransferRequest, EvmTrackedTransaction,
    EvmTransactionLifecycle, EvmTransactionSimulation,
};
pub use filesystem::{
    FilesystemAppliedChange, FilesystemEntry, FilesystemEntryKind, FilesystemFindPathKind,
    FilesystemFindPathQuery, FilesystemFindPathQueryResult, FilesystemFindPathsRequest,
    FilesystemFindPathsResult, FilesystemInspectImageRequest, FilesystemListRequest,
    FilesystemListResult, FilesystemMetadata, FilesystemReadTextRequest, FilesystemReadTextResult,
    FilesystemReplaceTextRequest, FilesystemScanMatch, FilesystemScanRequest, FilesystemScanResult,
    FilesystemSearchMatch, FilesystemSearchRequest, FilesystemSearchResult, FilesystemStatRequest,
    FilesystemTextReplacement, FilesystemUndoChangeRecord, FilesystemUndoFileResult,
    FilesystemUndoRequest, FilesystemUndoResult, FilesystemUndoStatus, FilesystemWriteResult,
    FilesystemWriteTextChange, FilesystemWriteTextRequest,
};
pub use generation::{
    ChatGenerationAccepted, ChatGenerationEvent, ChatGenerationPhase, FinishReason, GenerationEvent,
};
pub use model::{
    CatalogueCompanion, CatalogueModel, CatalogueModule, ExternalModelProgress, ModelCompanionKind,
    ModelDownloadEvent, ModelDownloadPhase, ModelDownloadRequest, ModelImportEvent,
    ModelImportReceipt, ModelInstallReceipt, ModelModuleSource, ModelPreferences,
    ModelRegistrationState, ModelSelection, SetConversationModelRequest, SetDefaultModelRequest,
};
pub use permission::{PermissionDecision, PermissionKind, PermissionOutcome, PermissionRequest};
pub use project::{
    CreateProjectRequest, ProjectSummary, ProjectWorkspaceRole, ProjectWorkspaceSummary,
    UpdateProjectWorkspacePermissionRequest,
};
pub use speech::{
    DictationAccepted, DictationBinding, DictationCommand, DictationEvent, DictationEventKind,
    DictationEventsRequest, DictationFailureCode, SpeechDownloadProgress, SpeechInstallState,
    SpeechModelRequest, SpeechModelView, StartDictationRequest,
};
pub use tool::{JsonValue, ToolName, ToolOutcome, ToolRequest, ToolResult, ToolResultPart};
pub use transaction::{
    SimulationState, TransactionAction, TransactionField, TransactionProposal,
    TransactionSimulation,
};
pub use validation::ValidateIpc;
pub use wallet::{PublicWalletState, WalletAvailability};
pub use workspace::{WorkspaceGrantSummary, WorkspacePermissionLevel};
pub use x402::{X402ApprovalDecision, X402ApprovalReviewOutcome};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Version of the frontend IPC contract.
pub const IPC_SCHEMA_VERSION: u16 = 35;
/// Maximum UTF-8 byte length of a user chat message.
pub const MAX_CHAT_TEXT_BYTES: usize = 64 * 1024;
/// Maximum number of managed assets attached to one user message.
pub const MAX_CHAT_ASSETS: usize = 4;
/// Maximum UTF-8 byte length of a user-visible conversation title.
pub const MAX_CONVERSATION_TITLE_BYTES: usize = 160;
/// Maximum UTF-8 byte length of a user-visible project name.
pub const MAX_PROJECT_NAME_BYTES: usize = 160;
/// Maximum serialized byte length of tool arguments.
pub const MAX_TOOL_ARGUMENT_BYTES: usize = 256 * 1024;
/// Maximum UTF-8 byte length of a streamed text or reasoning delta.
pub const MAX_GENERATION_DELTA_BYTES: usize = 32 * 1024;
/// Maximum UTF-8 byte length of a final generated response.
pub const MAX_FINAL_RESPONSE_BYTES: usize = 1024 * 1024;
/// Maximum UTF-8 byte length of a permission explanation.
pub const MAX_PERMISSION_REASON_BYTES: usize = 8 * 1024;
/// Maximum number of human-readable fields in a transaction proposal.
pub const MAX_TRANSACTION_FIELDS: usize = 64;

macro_rules! ipc_id {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize, TS)]
        #[serde(transparent)]
        #[ts(type = "string")]
        pub struct $name(
            /// Opaque identifier value.
            pub String,
        );
    };
}

ipc_id!(AssetId, "Opaque identifier for a managed local asset.");
ipc_id!(ConversationId, "Opaque identifier for a conversation.");
ipc_id!(ProjectId, "Opaque identifier for a project.");
ipc_id!(MessageId, "Opaque identifier for a conversation message.");
ipc_id!(
    PermissionRequestId,
    "Opaque identifier for a permission request."
);
ipc_id!(RequestId, "Opaque identifier for a generation request.");
ipc_id!(
    DictationSessionId,
    "Opaque identifier for a dictation session."
);
ipc_id!(ToolCallId, "Opaque identifier for one tool invocation.");
ipc_id!(
    TransactionProposalId,
    "Opaque identifier for a transaction proposal."
);
ipc_id!(WalletId, "Opaque identifier for public wallet metadata.");
ipc_id!(WorkspaceGrantId, "Opaque identifier for a workspace grant.");

/// State returned by the desktop shell health command.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppStatus {
    /// Current native runtime state.
    pub state: RuntimeState,
    /// IPC schema version understood by the native host.
    pub ipc_schema_version: u16,
}

impl AppStatus {
    /// Returns the healthy status for the current IPC schema.
    #[must_use]
    pub const fn ready() -> Self {
        Self {
            state: RuntimeState::Ready,
            ipc_schema_version: IPC_SCHEMA_VERSION,
        }
    }
}

/// Public native runtime state.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum RuntimeState {
    /// The Rust host is ready to accept allow-listed commands.
    Ready,
}

/// Stable frontend-safe error categories.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum IpcErrorCode {
    /// Input failed schema or semantic validation.
    InvalidRequest,
    /// Input exceeded a declared IPC limit.
    PayloadTooLarge,
    /// A requested operation or variant is not supported.
    Unsupported,
    /// The user or host permission policy did not authorize the exact effect.
    PermissionDenied,
    /// The requested resource falls outside the active user-authorized scope.
    ScopeCorrection,
    /// The operation does not advance the current accepted semantic plan step.
    PlanMismatch,
    /// The operation was cancelled without a privileged effect.
    Cancelled,
    /// A non-sensitive internal failure occurred.
    Internal,
}

/// Sanitized error returned across the frontend boundary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IpcError {
    /// Stable machine-readable category.
    pub code: IpcErrorCode,
    /// Human-readable message that contains no secret or internal diagnostic.
    pub message: String,
    /// Whether the caller may retry without changing the request.
    pub retryable: bool,
}

impl IpcError {
    /// Constructs an invalid-request error.
    #[must_use]
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self {
            code: IpcErrorCode::InvalidRequest,
            message: message.into(),
            retryable: false,
        }
    }

    /// Constructs an exact permission-denial error.
    #[must_use]
    pub fn permission_denied(message: impl Into<String>) -> Self {
        Self {
            code: IpcErrorCode::PermissionDenied,
            message: message.into(),
            retryable: false,
        }
    }

    /// Constructs a correctable user-scope error.
    #[must_use]
    pub fn scope_correction(message: impl Into<String>) -> Self {
        Self {
            code: IpcErrorCode::ScopeCorrection,
            message: message.into(),
            retryable: true,
        }
    }

    /// Constructs a correctable semantic-plan mismatch.
    #[must_use]
    pub fn plan_mismatch(message: impl Into<String>) -> Self {
        Self {
            code: IpcErrorCode::PlanMismatch,
            message: message.into(),
            retryable: true,
        }
    }

    /// Constructs a payload-too-large error.
    #[must_use]
    pub fn payload_too_large(message: impl Into<String>) -> Self {
        Self {
            code: IpcErrorCode::PayloadTooLarge,
            message: message.into(),
            retryable: false,
        }
    }

    /// Constructs a sanitized internal error.
    #[must_use]
    pub fn internal() -> Self {
        Self {
            code: IpcErrorCode::Internal,
            message: "The request could not be processed".to_owned(),
            retryable: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_status_has_the_expected_wire_shape() {
        let encoded = serde_json::to_value(AppStatus::ready());
        assert_eq!(
            encoded.map_err(|error| error.to_string()),
            Ok(serde_json::json!({
                "state": "ready",
                "ipcSchemaVersion": IPC_SCHEMA_VERSION
            }))
        );
    }

    #[test]
    fn active_tool_names_match_the_public_contract() {
        for (tool, expected) in [
            (ToolName::ComputationRun, "\"computation.run\""),
            (
                ToolName::ComputationRunProject,
                "\"computation.run_project\"",
            ),
            (ToolName::WebRetrieve, "\"web.retrieve\""),
            (ToolName::WebSearch, "\"web.search\""),
            (ToolName::FilesystemList, "\"filesystem.list\""),
            (ToolName::FilesystemFindPaths, "\"filesystem.find_paths\""),
            (
                ToolName::FilesystemCollectEvidence,
                "\"filesystem.collect_evidence\"",
            ),
            (ToolName::FilesystemStat, "\"filesystem.stat\""),
            (ToolName::FilesystemReadText, "\"filesystem.read_text\""),
            (
                ToolName::FilesystemInspectImage,
                "\"filesystem.inspect_image\"",
            ),
            (ToolName::FilesystemSearch, "\"filesystem.search\""),
            (ToolName::FilesystemScan, "\"filesystem.scan\""),
            (
                ToolName::FilesystemCreateDirectory,
                "\"filesystem.create_directory\"",
            ),
            (ToolName::FilesystemWriteText, "\"filesystem.write_text\""),
            (
                ToolName::FilesystemReplaceText,
                "\"filesystem.replace_text\"",
            ),
            (
                ToolName::FilesystemWriteDelimitedTable,
                "\"filesystem.write_delimited_table\"",
            ),
            (
                ToolName::OrchestrationTransformArtifact,
                "\"orchestration.transform_artifact\"",
            ),
            (
                ToolName::OrchestrationSubmitPlan,
                "\"orchestration.submit_plan\"",
            ),
            (
                ToolName::OrchestrationSubmitForecast,
                "\"orchestration.submit_forecast\"",
            ),
            (
                ToolName::OrchestrationCompleteStage,
                "\"orchestration.complete_stage\"",
            ),
            (
                ToolName::OrchestrationRefineStage,
                "\"orchestration.refine_stage\"",
            ),
            (
                ToolName::OrchestrationToolCorrection,
                "\"orchestration.tool_correction\"",
            ),
        ] {
            assert_eq!(
                serde_json::to_string(&tool).map_err(|error| error.to_string()),
                Ok(expected.to_owned())
            );
        }
    }
}
