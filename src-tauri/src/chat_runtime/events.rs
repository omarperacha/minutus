//! Trusted chat activity records and frontend event emission.

use ipc_types::{
    ChatGenerationEvent, ChatGenerationPhase, ConversationId, GenerationEvent, IpcError,
    IpcErrorCode, RequestId, TaskActivity, TaskActivityKind, TaskActivityStatus,
};
use tauri::Emitter;

use crate::app_state::CHAT_GENERATION_EVENT;

pub(super) fn tool_loop_limit_error() -> IpcError {
    IpcError {
        code: IpcErrorCode::Unsupported,
        message: "The response reached its workspace operation limit before completion".to_owned(),
        retryable: true,
    }
}

pub(crate) fn tool_activity(call_id: &str, tool_name: &str) -> TaskActivity {
    let (kind, title) = match tool_name {
        "computation.run" => (
            TaskActivityKind::PureComputation,
            "Ran isolated computation without project access",
        ),
        "computation.run_project" => (
            TaskActivityKind::ProjectComputation,
            "Computed over selected project files",
        ),
        "web.retrieve" => (
            TaskActivityKind::Search,
            "Retrieved an approved web resource",
        ),
        "web.search" => (TaskActivityKind::Search, "Searched the public web"),
        "wallet.request_transfer" => (
            TaskActivityKind::TransactionPreparation,
            "Requested a native-reviewed wallet transfer",
        ),
        "x402.request" => (
            TaskActivityKind::TransactionPreparation,
            "Requested human-approved x402 resource",
        ),
        "filesystem.scan" => (TaskActivityKind::FileRead, "Analyzed workspace files"),
        "filesystem.find_paths" => (TaskActivityKind::FileRead, "Discovered workspace paths"),
        "filesystem.collect_evidence" => {
            (TaskActivityKind::FileRead, "Collected workspace evidence")
        }
        "filesystem.write_text" => (TaskActivityKind::FileWrite, "Updated workspace files"),
        "filesystem.replace_text" => (TaskActivityKind::FileWrite, "Updated workspace file"),
        "filesystem.write_delimited_table" => {
            (TaskActivityKind::FileWrite, "Wrote delimited table")
        }
        "filesystem.create_directory" => {
            (TaskActivityKind::FileWrite, "Created workspace directory")
        }
        "orchestration.submit_plan" => (TaskActivityKind::Search, "Execution plan"),
        "orchestration.submit_forecast" => (TaskActivityKind::Search, "Forecast workspace work"),
        "orchestration.transform_artifact" => {
            (TaskActivityKind::FileWrite, "Transforming artifact")
        }
        "orchestration.complete_stage" => (TaskActivityKind::Search, "Completed a work stage"),
        "orchestration.recall" => (TaskActivityKind::Search, "Recalled conversation context"),
        "orchestration.search_tools" => (TaskActivityKind::Search, "Searched available task tools"),
        "orchestration.refine_stage" => (TaskActivityKind::Search, "Refined a work stage"),
        "orchestration.tool_correction" => {
            (TaskActivityKind::Search, "Corrected workspace operation")
        }
        "filesystem.inspect_image" => (TaskActivityKind::FileRead, "Inspected workspace image"),
        "filesystem.list"
        | "filesystem.stat"
        | "filesystem.read_text"
        | "filesystem.search"
        | "zeta.read_workspace_file"
        | "readWorkspaceFile" => (
            TaskActivityKind::FileRead,
            "Requested a workspace file read",
        ),
        "zeta.propose_workspace_change" | "proposeWorkspaceChange" => (
            TaskActivityKind::ProposedWrite,
            "Proposed a workspace change",
        ),
        "zeta.propose_transaction" | "proposeTransaction" => (
            TaskActivityKind::TransactionPreparation,
            "Requested transaction preparation",
        ),
        "zeta.lookup" | "lookup" | "zeta.fetch_url" | "fetchUrl" => {
            (TaskActivityKind::Search, "Requested a search")
        }
        _ => (TaskActivityKind::Search, "Requested an unavailable tool"),
    };
    TaskActivity {
        id: call_id.to_owned(),
        kind,
        title: title.to_owned(),
        detail: None,
        status: TaskActivityStatus::InProgress,
    }
}

pub(crate) fn next_sequence(sequence: &mut u32) -> u32 {
    let current = *sequence;
    *sequence = (*sequence).saturating_add(1);
    current
}

pub(crate) fn emit_chat_phase(
    app: &tauri::AppHandle,
    conversation_id: &ConversationId,
    request_id: &RequestId,
    sequence: &mut u32,
    phase: ChatGenerationPhase,
) {
    emit_chat_event(
        app,
        conversation_id,
        GenerationEvent::Status {
            request_id: request_id.clone(),
            sequence: next_sequence(sequence),
            phase,
        },
    );
}

pub(crate) fn emit_chat_event(
    app: &tauri::AppHandle,
    conversation_id: &ConversationId,
    event: GenerationEvent,
) {
    let _ = app.emit(
        CHAT_GENERATION_EVENT,
        ChatGenerationEvent {
            conversation_id: conversation_id.clone(),
            event,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_calls_map_to_closed_activity_categories() {
        assert_eq!(
            tool_activity("call-1", "zeta.read_workspace_file").kind,
            TaskActivityKind::FileRead
        );
        for tool in [
            "filesystem.list",
            "filesystem.find_paths",
            "filesystem.collect_evidence",
            "filesystem.stat",
            "filesystem.read_text",
            "filesystem.inspect_image",
            "filesystem.search",
            "filesystem.scan",
        ] {
            assert_eq!(
                tool_activity("filesystem-call", tool).kind,
                TaskActivityKind::FileRead
            );
        }
        assert_eq!(
            tool_activity("write-call", "filesystem.write_text").kind,
            TaskActivityKind::FileWrite
        );
        assert_eq!(
            tool_activity("replace-call", "filesystem.replace_text").kind,
            TaskActivityKind::FileWrite
        );
        assert_eq!(
            tool_activity("table-call", "filesystem.write_delimited_table").kind,
            TaskActivityKind::FileWrite
        );
        assert_eq!(
            tool_activity("call-2", "zeta.lookup").kind,
            TaskActivityKind::Search
        );
        assert_eq!(
            tool_activity("call-3", "zeta.propose_workspace_change").kind,
            TaskActivityKind::ProposedWrite
        );
        assert_eq!(
            tool_activity("call-4", "zeta.propose_transaction").kind,
            TaskActivityKind::TransactionPreparation
        );
        assert_eq!(
            tool_activity("x402", "x402.request").kind,
            TaskActivityKind::TransactionPreparation
        );
        assert_eq!(
            tool_activity("tool-search", "orchestration.search_tools").title,
            "Searched available task tools"
        );
        assert_eq!(
            tool_activity("pure", "computation.run").kind,
            TaskActivityKind::PureComputation
        );
        assert_eq!(
            tool_activity("project", "computation.run_project").kind,
            TaskActivityKind::ProjectComputation
        );
    }
}
