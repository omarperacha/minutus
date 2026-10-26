//! Desktop adapter for one freshly approved bounded static web retrieval.

use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration as StdDuration, Instant},
};

use agent_core::{
    AgentStateMachine, CancellationReason, LedgerPermissionDecision, PermissionMediator,
    ResultContent, RunCancellationReason, ToolOutcome as AgentToolOutcome, ToolResultEnvelope,
    ValidatedToolRequest,
};
use app_core::{
    AppCore, WebDiagnosticOutcome, authorize_web_request, prepare_web_tool_result,
    validate_web_tool_request, web_permission_scope, web_tool_registry,
};
use ipc_types::{
    AssetId, Conversation, IpcError, IpcErrorCode, JsonValue, ToolOutcome, ToolResultPart,
};
use model_runtime::ParsedToolCall;
use tauri::Manager as _;
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};
use web_retrieval::{SystemResolver, WebBroker, WebBrokerError, WebCancellation, WebRequest};

use crate::error_mapping::app_core_error;

use super::filesystem::ipc_json_value;

pub(super) struct ExecutedWebRetrieval {
    pub(super) outcome: ToolOutcome,
    pub(super) asset_id: Option<AssetId>,
    pub(super) injection_signal_count: usize,
}

pub(super) fn validate_call(
    call: &ParsedToolCall,
) -> Result<(ValidatedToolRequest, WebRequest), IpcError> {
    validate_web_tool_request(
        call.call_id.clone(),
        serde_json::Value::Object(call.arguments.clone().into_iter().collect()),
    )
    .map_err(|_| IpcError::invalid_request("The model returned invalid web arguments"))
}

#[expect(
    clippy::too_many_lines,
    reason = "web execution keeps approval, transport, inspection, and state transitions contiguous"
)]
pub(super) fn execute(
    app: &tauri::AppHandle,
    core: &AppCore,
    conversation: &Conversation,
    call: &ParsedToolCall,
    image_slot_available: bool,
    cancellation: &AtomicBool,
) -> Result<ExecutedWebRetrieval, IpcError> {
    if app
        .try_state::<tauri_plugin_dialog::Dialog<tauri::Wry>>()
        .is_none()
    {
        return Err(IpcError::permission_denied(
            "Web retrieval needs interactive approval, which is unavailable in this session",
        ));
    }
    let registry = web_tool_registry().map_err(|_| IpcError::internal())?;
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

    let scope = web_permission_scope(conversation.project_id.0.clone(), &request)
        .map_err(|_| IpcError::internal())?;
    let permission_id = format!("permission-{}", uuid::Uuid::now_v7());
    let mut mediator = PermissionMediator::default();
    mediator
        .request_interactive(&permission_id, &validated, scope.clone())
        .map_err(|_| IpcError::internal())?;
    let approved =
        !cancellation.load(Ordering::Acquire) && crate::web_approval::review(app, &request)?;
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
                "The web retrieval was cancelled"
            } else {
                "The web retrieval was not approved"
            }
            .to_owned(),
            retryable: false,
        });
    };
    let authorized = authorize_web_request(
        conversation.project_id.0.clone(),
        &validated,
        request.clone(),
        authorization,
    )
    .map_err(|_| IpcError::internal())?;
    let started_request = authorized
        .start(&mut state, &awaiting)
        .map_err(|_| IpcError::internal())?;

    let web_cancellation = WebCancellation::default();
    let watcher_cancellation = web_cancellation.clone();
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
        let response = tauri::async_runtime::block_on(started_request.execute(
            &WebBroker,
            &SystemResolver,
            &web_cancellation,
        ));
        watcher_done.store(true, Ordering::Release);
        let _ = watcher.join();
        response
    });
    core.record_web_execution_diagnostic(
        match &response {
            Ok(_) => WebDiagnosticOutcome::Succeeded,
            Err(WebBrokerError::Cancelled) => WebDiagnosticOutcome::Cancelled,
            Err(WebBrokerError::TargetRejected(_) | WebBrokerError::ResponseRejected(_)) => {
                WebDiagnosticOutcome::Rejected
            }
            Err(_) => WebDiagnosticOutcome::Failed,
        },
        u64::try_from(network_started.elapsed().as_millis()).unwrap_or(u64::MAX),
    );
    match response {
        Ok(response) => consume_response(
            core,
            conversation,
            &registry,
            &validated,
            &request,
            response,
            &mut state,
            image_slot_available,
        ),
        Err(error) => {
            record_broker_terminal(&registry, &validated, &mut state, error)?;
            Err(web_broker_error(error))
        }
    }
}

fn record_broker_terminal(
    registry: &agent_core::ToolRegistry,
    validated: &ValidatedToolRequest,
    state: &mut AgentStateMachine,
    error: WebBrokerError,
) -> Result<(), IpcError> {
    let envelope = if error == WebBrokerError::Cancelled {
        registry.cancelled(validated, CancellationReason::UserRequested)
    } else if matches!(
        error,
        WebBrokerError::DnsDeadline
            | WebBrokerError::ConnectionOrHeaderDeadline
            | WebBrokerError::BodyIdleDeadline
            | WebBrokerError::TotalDeadline
    ) {
        registry.cancelled(validated, CancellationReason::DeadlineExceeded)
    } else {
        registry.failed(
            validated,
            agent_core::ToolFailure::new("web_retrieval_failed", error.to_string(), false)
                .map_err(|_| IpcError::internal())?,
        )
    }
    .map_err(|_| IpcError::internal())?;
    state
        .return_tool_result(&state.transition_token(), &envelope)
        .map_err(|_| IpcError::internal())?;
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "response consumption binds the exact approved request and trusted execution state"
)]
fn consume_response(
    core: &AppCore,
    conversation: &Conversation,
    registry: &agent_core::ToolRegistry,
    validated: &ValidatedToolRequest,
    request: &WebRequest,
    response: web_retrieval::WebBrokerResponse,
    state: &mut AgentStateMachine,
    image_slot_available: bool,
) -> Result<ExecutedWebRetrieval, IpcError> {
    let compact_id = format!("web-{}", uuid::Uuid::now_v7());
    let (created_at, expires_at) = retention_timestamps(OffsetDateTime::now_utc())?;
    let prepared = prepare_web_tool_result(
        core,
        registry,
        validated,
        request,
        response,
        &compact_id,
        &conversation.id.0,
        &conversation.project_id,
        &created_at,
        &expires_at,
    )
    .map_err(|_| IpcError::internal())?;
    let (envelope, record, indicators, _reuse_digest) = prepared.into_parts();
    let asset_id = envelope_asset_id(&envelope);
    if !image_slot_available && let Some(asset_id) = &asset_id {
        core.discard_unreferenced_image(asset_id)
            .map_err(app_core_error)?;
        let failure = registry
            .failed(
                validated,
                agent_core::ToolFailure::new(
                    "image_budget_exhausted",
                    "The conversation image limit was reached",
                    false,
                )
                .map_err(|_| IpcError::internal())?,
            )
            .map_err(|_| IpcError::internal())?;
        state
            .return_tool_result(&state.transition_token(), &failure)
            .map_err(|_| IpcError::internal())?;
        return Err(IpcError::payload_too_large(
            "The conversation already contains the maximum number of images",
        ));
    }
    core.with_storage(|storage| storage.save_compact_tool_result(&record))
        .map_err(app_core_error)?;
    state
        .return_tool_result(&state.transition_token(), &envelope)
        .map_err(|_| IpcError::internal())?;
    Ok(ExecutedWebRetrieval {
        outcome: ipc_outcome(&envelope)?,
        asset_id,
        injection_signal_count: indicators.len(),
    })
}

pub(super) fn retention_timestamps(now: OffsetDateTime) -> Result<(String, String), IpcError> {
    let created = now
        .replace_nanosecond(0)
        .map_err(|_| IpcError::internal())?;
    Ok((
        created.format(&Rfc3339).map_err(|_| IpcError::internal())?,
        (created + Duration::days(7))
            .format(&Rfc3339)
            .map_err(|_| IpcError::internal())?,
    ))
}

fn web_broker_error(error: WebBrokerError) -> IpcError {
    IpcError {
        code: if error == WebBrokerError::Cancelled {
            IpcErrorCode::Cancelled
        } else {
            IpcErrorCode::Unsupported
        },
        message: error.to_string(),
        retryable: false,
    }
}

fn envelope_asset_id(envelope: &ToolResultEnvelope) -> Option<AssetId> {
    let AgentToolOutcome::Completed { content } = envelope.outcome() else {
        return None;
    };
    content.iter().find_map(|part| match part {
        ResultContent::ValidatedAsset { asset } => Some(AssetId(asset.asset_id().to_owned())),
        _ => None,
    })
}

pub(super) fn ipc_outcome(envelope: &ToolResultEnvelope) -> Result<ToolOutcome, IpcError> {
    Ok(match envelope.outcome() {
        AgentToolOutcome::Completed { content } => ToolOutcome::Completed {
            content: content
                .iter()
                .map(|part| match part {
                    ResultContent::Text { text } => Ok(ToolResultPart::Text { text: text.clone() }),
                    ResultContent::Json { value } => Ok(ToolResultPart::Json {
                        value: ipc_json_value(value)?,
                    }),
                    ResultContent::OpaqueLocalReference { reference } => Ok(ToolResultPart::Json {
                        value: JsonValue::Object(BTreeMap::from([(
                            "reference".to_owned(),
                            JsonValue::Object(BTreeMap::from([
                                (
                                    "id".to_owned(),
                                    JsonValue::String(reference.id().to_owned()),
                                ),
                                (
                                    "kind".to_owned(),
                                    JsonValue::String("externalResponse".to_owned()),
                                ),
                            ])),
                        )])),
                    }),
                    ResultContent::ValidatedAsset { asset } => Ok(ToolResultPart::Asset {
                        asset_id: AssetId(asset.asset_id().to_owned()),
                    }),
                })
                .collect::<Result<_, IpcError>>()?,
        },
        AgentToolOutcome::Failed { failure } => ToolOutcome::Failed {
            error: IpcError {
                code: IpcErrorCode::Unsupported,
                message: failure.message().to_owned(),
                retryable: false,
            },
        },
        AgentToolOutcome::Cancelled { reason } => ToolOutcome::Failed {
            error: IpcError {
                code: IpcErrorCode::Cancelled,
                message: match reason {
                    CancellationReason::UserRequested => "The web retrieval was cancelled",
                    CancellationReason::DeadlineExceeded => "The web retrieval timed out",
                    CancellationReason::PermissionWithdrawn => "Web permission was withdrawn",
                    CancellationReason::HostShutdown => "The application stopped the retrieval",
                }
                .to_owned(),
                retryable: false,
            },
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crash_path_errors_are_closed_and_contain_no_remote_diagnostics() {
        for error in [
            WebBrokerError::DnsDeadline,
            WebBrokerError::ConnectionOrHeaderDeadline,
            WebBrokerError::BodyIdleDeadline,
            WebBrokerError::TotalDeadline,
            WebBrokerError::TargetRejected(web_retrieval::TargetValidationError::NonPublicAddress),
            WebBrokerError::ResponseRejected(
                web_retrieval::ResponseValidationError::HeaderAggregateLimit,
            ),
            WebBrokerError::TransportFailed,
        ] {
            let projected = serde_json::to_string(&web_broker_error(error)).unwrap_or_default();
            for forbidden in [
                "https://",
                "private_token",
                "Set-Cookie",
                "session=",
                "certificate chain",
                "10.0.0.1",
            ] {
                assert!(!projected.contains(forbidden));
            }
        }
    }
}
