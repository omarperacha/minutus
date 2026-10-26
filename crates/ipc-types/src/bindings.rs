//! Deterministic single-file TypeScript binding generation.

use ts_rs::{Config, TS};

use crate::{
    AppStatus, AssetId, CatalogueCompanion, CatalogueModel, CatalogueModule, ChatEffort,
    ChatGenerationAccepted, ChatGenerationEvent, ChatGenerationPhase, ChatRequest, Conversation,
    ConversationId, ConversationSummary, CreateProjectRequest, DictationAccepted, DictationBinding,
    DictationCommand, DictationEvent, DictationEventKind, DictationEventsRequest,
    DictationFailureCode, DictationSessionId, EvmApprovalDecision, EvmApprovalReviewOutcome,
    EvmAssetBalance, EvmAuthorizationDecodeRequest, EvmAuthorizationField, EvmAuthorizationKind,
    EvmAuthorizationPayloadKind, EvmAuthorizationStatus, EvmAuthorizationWarning,
    EvmAuthorizationWarningCode, EvmAuthorizationWarningSeverity, EvmBalanceFreshness,
    EvmBalanceRequest, EvmBalanceResponse, EvmBalanceSnapshot, EvmDecodedAuthorization,
    EvmDecodedTokenTransfer, EvmNativeTransferCandidate, EvmNativeTransferRequest, EvmNetwork,
    EvmRpcNetworkSetting, EvmRpcSettings, EvmSimulationComparison, EvmSimulationEffect,
    EvmSimulationEffectKind, EvmSimulationState, EvmTokenTransferCandidate,
    EvmTokenTransferRequest, EvmTrackedTransaction, EvmTransactionLifecycle,
    EvmTransactionSimulation, ExternalModelProgress, FilesystemAppliedChange, FilesystemEntry,
    FilesystemEntryKind, FilesystemFindPathKind, FilesystemFindPathQuery,
    FilesystemFindPathQueryResult, FilesystemFindPathsRequest, FilesystemFindPathsResult,
    FilesystemInspectImageRequest, FilesystemListRequest, FilesystemListResult, FilesystemMetadata,
    FilesystemReadTextRequest, FilesystemReadTextResult, FilesystemReplaceTextRequest,
    FilesystemScanMatch, FilesystemScanRequest, FilesystemScanResult, FilesystemSearchMatch,
    FilesystemSearchRequest, FilesystemSearchResult, FilesystemStatRequest,
    FilesystemTextReplacement, FilesystemUndoChangeRecord, FilesystemUndoFileResult,
    FilesystemUndoRequest, FilesystemUndoResult, FilesystemUndoStatus, FilesystemWriteResult,
    FilesystemWriteTextChange, FilesystemWriteTextRequest, FinishReason, GenerationEvent,
    ImageAttachmentReceipt, IpcError, IpcErrorCode, JsonValue, Message, MessageId, MessagePart,
    MessageRole, ModelCompanionKind, ModelDownloadEvent, ModelDownloadPhase, ModelDownloadRequest,
    ModelImportEvent, ModelImportReceipt, ModelInstallReceipt, ModelModuleSource, ModelPreferences,
    ModelRegistrationState, ModelSelection, PermissionDecision, PermissionKind, PermissionOutcome,
    PermissionRequest, PermissionRequestId, ProjectId, ProjectSummary, ProjectWorkspaceRole,
    ProjectWorkspaceSummary, PublicWalletState, RenameConversationRequest, RequestId, RuntimeState,
    SetConversationModelRequest, SetDefaultModelRequest, SimulationState, StartDictationRequest,
    TaskActivity, TaskActivityKind, TaskActivityStatus, ToolCallId, ToolName, ToolOutcome,
    ToolRequest, ToolResult, ToolResultPart, TransactionAction, TransactionField,
    TransactionProposal, TransactionProposalId, TransactionSimulation,
    UpdateProjectWorkspacePermissionRequest, WalletAvailability, WalletId, WorkspaceGrantId,
    WorkspacePermissionLevel, X402ApprovalDecision, X402ApprovalReviewOutcome,
};

/// Generates the complete TypeScript IPC contract from the Rust definitions.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn typescript_bindings() -> String {
    let config = Config::default();
    let declarations = [
        AssetId::decl(&config),
        ConversationId::decl(&config),
        ProjectId::decl(&config),
        MessageId::decl(&config),
        PermissionRequestId::decl(&config),
        RequestId::decl(&config),
        DictationSessionId::decl(&config),
        ToolCallId::decl(&config),
        TransactionProposalId::decl(&config),
        WalletId::decl(&config),
        WorkspaceGrantId::decl(&config),
        CreateProjectRequest::decl(&config),
        ProjectSummary::decl(&config),
        ProjectWorkspaceRole::decl(&config),
        ProjectWorkspaceSummary::decl(&config),
        UpdateProjectWorkspacePermissionRequest::decl(&config),
        EvmNetwork::decl(&config),
        EvmRpcNetworkSetting::decl(&config),
        EvmRpcSettings::decl(&config),
        EvmBalanceRequest::decl(&config),
        EvmBalanceFreshness::decl(&config),
        EvmAssetBalance::decl(&config),
        EvmBalanceSnapshot::decl(&config),
        EvmBalanceResponse::decl(&config),
        EvmNativeTransferRequest::decl(&config),
        EvmNativeTransferCandidate::decl(&config),
        EvmTokenTransferRequest::decl(&config),
        EvmDecodedTokenTransfer::decl(&config),
        EvmTokenTransferCandidate::decl(&config),
        EvmApprovalDecision::decl(&config),
        EvmApprovalReviewOutcome::decl(&config),
        X402ApprovalDecision::decl(&config),
        X402ApprovalReviewOutcome::decl(&config),
        EvmSimulationState::decl(&config),
        EvmSimulationComparison::decl(&config),
        EvmSimulationEffectKind::decl(&config),
        EvmSimulationEffect::decl(&config),
        EvmTransactionSimulation::decl(&config),
        EvmTransactionLifecycle::decl(&config),
        EvmTrackedTransaction::decl(&config),
        EvmAuthorizationPayloadKind::decl(&config),
        EvmAuthorizationDecodeRequest::decl(&config),
        EvmAuthorizationKind::decl(&config),
        EvmAuthorizationStatus::decl(&config),
        EvmAuthorizationWarningSeverity::decl(&config),
        EvmAuthorizationWarningCode::decl(&config),
        EvmAuthorizationField::decl(&config),
        EvmAuthorizationWarning::decl(&config),
        EvmDecodedAuthorization::decl(&config),
        WorkspacePermissionLevel::decl(&config),
        FilesystemListRequest::decl(&config),
        FilesystemFindPathKind::decl(&config),
        FilesystemFindPathQuery::decl(&config),
        FilesystemFindPathsRequest::decl(&config),
        FilesystemStatRequest::decl(&config),
        FilesystemReadTextRequest::decl(&config),
        FilesystemInspectImageRequest::decl(&config),
        FilesystemSearchRequest::decl(&config),
        FilesystemScanRequest::decl(&config),
        FilesystemWriteTextChange::decl(&config),
        FilesystemWriteTextRequest::decl(&config),
        FilesystemTextReplacement::decl(&config),
        FilesystemReplaceTextRequest::decl(&config),
        FilesystemEntryKind::decl(&config),
        FilesystemEntry::decl(&config),
        FilesystemListResult::decl(&config),
        FilesystemFindPathQueryResult::decl(&config),
        FilesystemFindPathsResult::decl(&config),
        FilesystemMetadata::decl(&config),
        FilesystemReadTextResult::decl(&config),
        FilesystemSearchMatch::decl(&config),
        FilesystemSearchResult::decl(&config),
        FilesystemScanMatch::decl(&config),
        FilesystemScanResult::decl(&config),
        FilesystemAppliedChange::decl(&config),
        FilesystemWriteResult::decl(&config),
        FilesystemUndoRequest::decl(&config),
        FilesystemUndoStatus::decl(&config),
        FilesystemUndoFileResult::decl(&config),
        FilesystemUndoResult::decl(&config),
        FilesystemUndoChangeRecord::decl(&config),
        RuntimeState::decl(&config),
        AppStatus::decl(&config),
        IpcErrorCode::decl(&config),
        IpcError::decl(&config),
        ImageAttachmentReceipt::decl(&config),
        ModelCompanionKind::decl(&config),
        CatalogueCompanion::decl(&config),
        ModelRegistrationState::decl(&config),
        ExternalModelProgress::decl(&config),
        ModelModuleSource::decl(&config),
        CatalogueModule::decl(&config),
        CatalogueModel::decl(&config),
        crate::SpeechModelRequest::decl(&config),
        crate::SpeechDownloadProgress::decl(&config),
        crate::SpeechInstallState::decl(&config),
        crate::SpeechModelView::decl(&config),
        DictationBinding::decl(&config),
        StartDictationRequest::decl(&config),
        DictationAccepted::decl(&config),
        DictationCommand::decl(&config),
        DictationEventsRequest::decl(&config),
        DictationFailureCode::decl(&config),
        DictationEventKind::decl(&config),
        DictationEvent::decl(&config),
        ModelSelection::decl(&config),
        ModelPreferences::decl(&config),
        SetDefaultModelRequest::decl(&config),
        SetConversationModelRequest::decl(&config),
        ModelDownloadRequest::decl(&config),
        ModelDownloadPhase::decl(&config),
        ModelDownloadEvent::decl(&config),
        ModelInstallReceipt::decl(&config),
        ModelImportEvent::decl(&config),
        ModelImportReceipt::decl(&config),
        JsonValue::decl(&config),
        TaskActivityKind::decl(&config),
        TaskActivityStatus::decl(&config),
        TaskActivity::decl(&config),
        ToolName::decl(&config),
        ToolRequest::decl(&config),
        ToolResultPart::decl(&config),
        ToolOutcome::decl(&config),
        ToolResult::decl(&config),
        MessageRole::decl(&config),
        MessagePart::decl(&config),
        Message::decl(&config),
        ConversationSummary::decl(&config),
        Conversation::decl(&config),
        RenameConversationRequest::decl(&config),
        ChatEffort::decl(&config),
        ChatRequest::decl(&config),
        ChatGenerationAccepted::decl(&config),
        ChatGenerationEvent::decl(&config),
        ChatGenerationPhase::decl(&config),
        FinishReason::decl(&config),
        GenerationEvent::decl(&config),
        PermissionKind::decl(&config),
        PermissionRequest::decl(&config),
        PermissionOutcome::decl(&config),
        PermissionDecision::decl(&config),
        WalletAvailability::decl(&config),
        PublicWalletState::decl(&config),
        TransactionAction::decl(&config),
        TransactionField::decl(&config),
        SimulationState::decl(&config),
        TransactionSimulation::decl(&config),
        TransactionProposal::decl(&config),
    ];

    let body = render_declarations(declarations);

    format!(
        "// This file is generated by ipc-types. Do not edit manually.\n\
         // Run: pnpm bindings:generate\n\n{body}\n"
    )
}

fn render_declarations<const N: usize>(declarations: [String; N]) -> String {
    declarations
        .into_iter()
        .map(|declaration| {
            let declaration = declaration
                .lines()
                .map(str::trim_end)
                .collect::<Vec<_>>()
                .join("\n");
            format!("export {declaration}")
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontend_contract_has_no_secret_bearing_fields() {
        let bindings = typescript_bindings();
        for forbidden in [
            "mnemonic",
            "privateKey",
            "recoveryPhrase",
            "seedPhrase",
            "signedTransaction",
            "signingPayload",
            "bookmarkData",
            "canonicalRoot",
        ] {
            assert!(
                !bindings.contains(forbidden),
                "frontend contract contains forbidden field: {forbidden}"
            );
        }
    }
}
