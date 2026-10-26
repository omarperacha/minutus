//! Desktop adapter for one freshly approved bounded Brave Search query.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration as StdDuration, Instant},
};

use agent_core::{
    AgentStateMachine, CancellationReason, LedgerPermissionDecision, PermissionMediator,
    RunCancellationReason, ValidatedToolRequest,
};
use app_core::{
    AppCore, WebDiagnosticOutcome, authorize_web_search_request, prepare_web_search_tool_result,
    validate_web_search_tool_request, web_search_permission_scope, web_search_tool_registry,
};
use ipc_types::{Conversation, IpcError, IpcErrorCode, ToolOutcome};
use model_runtime::ParsedToolCall;
use tauri::Manager as _;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
use time::OffsetDateTime;
use web_retrieval::{
    SearchBroker, SearchBrokerError, SearchRequest, SystemResolver, WebCancellation,
};

use crate::error_mapping::app_core_error;

pub(super) struct ExecutedWebSearch {
    pub(super) outcome: ToolOutcome,
    pub(super) injection_signal_count: usize,
}

pub(super) fn validate_call(
    call: &ParsedToolCall,
) -> Result<(ValidatedToolRequest, SearchRequest), IpcError> {
    validate_web_search_tool_request(
        call.call_id.clone(),
        serde_json::Value::Object(call.arguments.clone().into_iter().collect()),
    )
    .map_err(|_| IpcError::invalid_request("The model returned invalid web search arguments"))
}

#[allow(clippy::too_many_lines)]
pub(super) fn execute(
    app: &tauri::AppHandle,
    core: &AppCore,
    conversation: &Conversation,
    call: &ParsedToolCall,
    cancellation: &AtomicBool,
) -> Result<ExecutedWebSearch, IpcError> {
    if app
        .try_state::<tauri_plugin_dialog::Dialog<tauri::Wry>>()
        .is_none()
    {
        return Err(IpcError::permission_denied(
            "Web search needs interactive approval, which is unavailable in this session",
        ));
    }
    let credential = crate::search_credentials::load()?;
    let registry = web_search_tool_registry().map_err(|_| IpcError::internal())?;
    let (validated, request) = validate_call(call)?;
    let mut state = AgentStateMachine::start(
        format!("run-{}", uuid::Uuid::now_v7()),
        format!("generation-{}", uuid::Uuid::now_v7()),
    )
    .map_err(|_| IpcError::internal())?;
    let requested = state
        .request_tool(&state.transition_token(), &validated)
        .map_err(|_| IpcError::internal())?;
    let awaiting = state
        .await_permission(&requested)
        .map_err(|_| IpcError::internal())?;
    let scope = web_search_permission_scope(conversation.project_id.0.clone(), &request)
        .map_err(|_| IpcError::internal())?;
    let permission_id = format!("permission-{}", uuid::Uuid::now_v7());
    let mut mediator = PermissionMediator::default();
    let prompt = mediator
        .request_interactive(&permission_id, &validated, scope.clone())
        .map_err(|_| IpcError::internal())?;
    let approved = !cancellation.load(Ordering::Acquire)
        && app
            .dialog()
            .message(format!(
                "Allow this one-time Brave Search request?\n\n{}\n\nTitles and snippets are untrusted. Result pages will not be opened automatically.",
                prompt.resources.join("\n")
            ))
            .title("Allow web search?")
            .buttons(MessageDialogButtons::OkCancelCustom(
                "Allow once".to_owned(),
                "Deny".to_owned(),
            ))
            .blocking_show();
    let decision = if approved && !cancellation.load(Ordering::Acquire) {
        LedgerPermissionDecision::Approved
    } else if cancellation.load(Ordering::Acquire) {
        LedgerPermissionDecision::Cancelled
    } else {
        LedgerPermissionDecision::Denied
    };
    let receipt = mediator
        .decide(&permission_id, &validated, &scope, decision)
        .map_err(|_| IpcError::internal())?;
    core.record_web_permission_diagnostic(match decision {
        LedgerPermissionDecision::Approved => WebDiagnosticOutcome::Succeeded,
        LedgerPermissionDecision::Cancelled => WebDiagnosticOutcome::Cancelled,
        LedgerPermissionDecision::Denied
        | LedgerPermissionDecision::Dismissed
        | LedgerPermissionDecision::Expired
        | LedgerPermissionDecision::Pending => WebDiagnosticOutcome::Rejected,
    });
    let Some(authorization) = receipt.authorization else {
        state
            .cancel(&awaiting, RunCancellationReason::UserRequested)
            .map_err(|_| IpcError::internal())?;
        return Err(IpcError {
            code: if decision == LedgerPermissionDecision::Cancelled {
                IpcErrorCode::Cancelled
            } else {
                IpcErrorCode::PermissionDenied
            },
            message: if decision == LedgerPermissionDecision::Cancelled {
                "The web search was cancelled"
            } else {
                "The web search was not approved"
            }
            .to_owned(),
            retryable: false,
        });
    };
    let started = authorize_web_search_request(
        conversation.project_id.0.clone(),
        &validated,
        request,
        authorization,
    )
    .map_err(|_| IpcError::internal())?
    .start(&mut state, &awaiting)
    .map_err(|_| IpcError::internal())?;

    let search_cancellation = WebCancellation::default();
    let watcher_cancellation = search_cancellation.clone();
    let watcher_done = Arc::new(AtomicBool::new(false));
    let watcher_finished = Arc::clone(&watcher_done);
    let network_started = Instant::now();
    let response = thread::scope(|scope| {
        let watcher = scope.spawn(|| {
            while !watcher_finished.load(Ordering::Acquire) {
                if cancellation.load(Ordering::Acquire) {
                    watcher_cancellation.cancel();
                    break;
                }
                thread::sleep(StdDuration::from_millis(10));
            }
        });
        let response = tauri::async_runtime::block_on(started.execute(
            &SearchBroker,
            &credential,
            &SystemResolver,
            &search_cancellation,
        ));
        watcher_done.store(true, Ordering::Release);
        let _ = watcher.join();
        response
    });
    core.record_web_execution_diagnostic(
        match &response {
            Ok(_) => WebDiagnosticOutcome::Succeeded,
            Err(SearchBrokerError::Cancelled) => WebDiagnosticOutcome::Cancelled,
            Err(
                SearchBrokerError::TargetRejected(_)
                | SearchBrokerError::AuthenticationRejected
                | SearchBrokerError::ProviderRejected
                | SearchBrokerError::PaymentRequired,
            ) => WebDiagnosticOutcome::Rejected,
            Err(_) => WebDiagnosticOutcome::Failed,
        },
        u64::try_from(network_started.elapsed().as_millis()).unwrap_or(u64::MAX),
    );
    match response {
        Ok(response) => {
            let compact_id = format!("search-{}", uuid::Uuid::now_v7());
            let (created_at, expires_at) =
                super::web::retention_timestamps(OffsetDateTime::now_utc())?;
            let prepared = prepare_web_search_tool_result(
                &registry,
                &validated,
                &response,
                &compact_id,
                &conversation.id.0,
                &conversation.project_id,
                &created_at,
                &expires_at,
            )
            .map_err(|_| IpcError::internal())?;
            let (envelope, record, indicators) = prepared.into_parts();
            core.with_storage(|storage| storage.save_compact_tool_result(&record))
                .map_err(app_core_error)?;
            state
                .return_tool_result(&state.transition_token(), &envelope)
                .map_err(|_| IpcError::internal())?;
            Ok(ExecutedWebSearch {
                outcome: super::web::ipc_outcome(&envelope)?,
                injection_signal_count: indicators.len(),
            })
        }
        Err(error) => {
            let envelope = if error == SearchBrokerError::Cancelled {
                registry.cancelled(&validated, CancellationReason::UserRequested)
            } else if error == SearchBrokerError::Deadline {
                registry.cancelled(&validated, CancellationReason::DeadlineExceeded)
            } else {
                registry.failed(
                    &validated,
                    agent_core::ToolFailure::new("web_search_failed", error.to_string(), false)
                        .map_err(|_| IpcError::internal())?,
                )
            }
            .map_err(|_| IpcError::internal())?;
            state
                .return_tool_result(&state.transition_token(), &envelope)
                .map_err(|_| IpcError::internal())?;
            Err(search_broker_error(error))
        }
    }
}

fn search_broker_error(error: SearchBrokerError) -> IpcError {
    IpcError {
        code: if error == SearchBrokerError::Cancelled {
            IpcErrorCode::Cancelled
        } else {
            IpcErrorCode::Unsupported
        },
        message: error.to_string(),
        retryable: matches!(
            error,
            SearchBrokerError::RateLimited | SearchBrokerError::Deadline
        ),
    }
}
