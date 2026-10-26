//! Trusted adapter from model calls to the isolated one-shot computation runner.

use std::{collections::BTreeMap, path::PathBuf, sync::atomic::AtomicBool, time::Instant};

use agent_core::{
    AgentStateMachine, CancellationReason, DurableCapability, PermissionMediator,
    PermissionResourceKind, PermissionResourceScope, ProjectCapability, ResultContent,
    ToolOutcome as AgentToolOutcome, ToolResultEnvelope, UntrustedToolRequest,
    ValidatedToolRequest,
};
use app_core::{
    AppCore, ComputationDiagnosticStage, PURE_COMPUTATION_TOOL, ProjectComputationInputReference,
    ProjectComputationInputSnapshot, computation_reuse_digest, computation_runner_error_status,
    computation_terminal_status, computation_tool_registry, prepare_computation_result,
};
use computation_runtime::{
    CONTRACT_VERSION, ComputationRequest, ComputationRunner, InputFormat, OutputExpectation,
    OutputKind,
};
use ipc_types::{Conversation, IpcError, IpcErrorCode, JsonValue, ToolOutcome, ToolResultPart};
use model_runtime::ParsedToolCall;
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{
    error_mapping::{app_core_error, bookmark_access_error},
    workspace_bookmark,
};

use super::{ChatToolWorkspace, filesystem::ipc_json_value};

pub(super) struct ExecutedComputation {
    pub(super) outcome: ToolOutcome,
    pub(super) reuse_digest: String,
}

pub(super) fn validate_call(call: &ParsedToolCall) -> Result<ValidatedToolRequest, IpcError> {
    let validated = computation_tool_registry()
        .and_then(|registry| {
            registry.validate_request(UntrustedToolRequest {
                call_id: call.call_id.clone(),
                name: call.name.clone(),
                arguments: serde_json::Value::Object(call.arguments.clone().into_iter().collect()),
            })
        })
        .map_err(|_| IpcError::invalid_request("The model returned invalid tool arguments"))?;
    output_expectation(call)?;
    if call.name != PURE_COMPUTATION_TOOL {
        input_references(call)?;
    }
    Ok(validated)
}

#[expect(
    clippy::too_many_lines,
    reason = "the computation state machine keeps validation and every trusted transition contiguous"
)]
pub(super) fn execute(
    core: &AppCore,
    conversation: &Conversation,
    workspace: Option<&ChatToolWorkspace>,
    call: &ParsedToolCall,
    cancellation: &AtomicBool,
) -> Result<ExecutedComputation, IpcError> {
    let started = Instant::now();
    let registry = computation_tool_registry().map_err(|_| IpcError::internal())?;
    let validated = validate_call(call)?;
    let source = required_string(call, "source")?.to_owned();
    let output = output_expectation(call)?;
    let compact_id = format!("computation-{}", uuid::Uuid::now_v7());
    let request_id = format!("request-{}", uuid::Uuid::now_v7());
    let (created_at, expires_at) = retention_timestamps(OffsetDateTime::now_utc())?;

    let mut state = AgentStateMachine::start(
        format!("run-{}", uuid::Uuid::now_v7()),
        format!("generation-{}", uuid::Uuid::now_v7()),
    )
    .map_err(|_| IpcError::internal())?;
    let requested = state
        .request_tool(&state.transition_token(), &validated)
        .map_err(|_| IpcError::internal())?;

    let runner = ComputationRunner::new(runner_executable());
    let (request, snapshot, status, output_value) = if call.name == PURE_COMPUTATION_TOOL {
        let scope = PermissionResourceScope::new(
            conversation.project_id.0.clone(),
            PermissionResourceKind::Project,
            "no-project-authority",
            CONTRACT_VERSION,
            vec!["pure-computation".to_owned()],
        )
        .map_err(|_| IpcError::internal())?;
        let authorization = PermissionMediator::default()
            .authorize_without_capability(&validated, scope)
            .map_err(|_| IpcError::internal())?;
        state
            .start_capability_tool(&requested, authorization, true)
            .map_err(|_| IpcError::internal())?;
        let request = ComputationRequest::new(request_id, source, Vec::new(), output)
            .map_err(|_| IpcError::invalid_request("The model returned invalid tool arguments"))?;
        let (status, output) = run(&runner, &request, cancellation);
        (request, None, status, output)
    } else {
        let workspace = workspace.ok_or_else(|| {
            IpcError::invalid_request("The requested project workspace is unavailable")
        })?;
        let snapshot = bind_snapshot(core, conversation, workspace, call)?;
        let scope = PermissionResourceScope::new(
            conversation.project_id.0.clone(),
            PermissionResourceKind::Workspace,
            workspace.grant_id.0.clone(),
            snapshot.capability_revision(),
            snapshot
                .bindings()
                .iter()
                .map(|binding| binding.relative_path().to_owned())
                .collect(),
        )
        .map_err(|_| IpcError::internal())?;
        let capability = DurableCapability::new(
            conversation.project_id.0.clone(),
            ProjectCapability::WorkspaceRead,
            workspace.grant_id.0.clone(),
            snapshot.capability_revision(),
        )
        .map_err(|_| IpcError::internal())?;
        let authorization = PermissionMediator::default()
            .authorize_capability(&validated, scope, &capability)
            .map_err(|_| IpcError::invalid_request("Project read access is unavailable"))?;
        state
            .start_capability_tool(&requested, authorization, true)
            .map_err(|_| IpcError::internal())?;
        let execution = core
            .with_admitted_project_computation_inputs(&snapshot, |inputs| {
                let request = ComputationRequest::new(request_id, source, inputs.to_vec(), output);
                request.map(|request| {
                    let (status, output) = run(&runner, &request, cancellation);
                    (request, status, output)
                })
            })
            .map_err(app_core_error)?
            .map_err(|_| IpcError::invalid_request("The model returned invalid tool arguments"))?;
        (execution.0, Some(snapshot), execution.1, execution.2)
    };

    let reuse_digest =
        computation_reuse_digest(&request, snapshot.as_ref()).map_err(|_| IpcError::internal())?;
    let _ = core.record_computation_diagnostic(
        status,
        u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    );
    let prepared = prepare_computation_result(
        &registry,
        &validated,
        &request,
        status,
        output_value.as_ref(),
        snapshot.as_ref(),
        &compact_id,
        &conversation.id.0,
        &conversation.project_id.0,
        &created_at,
        &expires_at,
    )
    .map_err(|_| {
        record_adapter_failure(
            core,
            ComputationDiagnosticStage::PrepareResult,
            started.elapsed(),
        );
        IpcError::internal()
    })?;
    let (envelope, record, _) = prepared.into_parts();
    core.with_storage(|storage| storage.save_compact_tool_result(&record))
        .map_err(|error| {
            record_adapter_failure(
                core,
                ComputationDiagnosticStage::PersistResult,
                started.elapsed(),
            );
            app_core_error(error)
        })?;
    state
        .return_tool_result(&state.transition_token(), &envelope)
        .map_err(|_| {
            record_adapter_failure(
                core,
                ComputationDiagnosticStage::ReturnAgentResult,
                started.elapsed(),
            );
            IpcError::internal()
        })?;
    let outcome = ipc_outcome(&envelope).inspect_err(|_error| {
        record_adapter_failure(
            core,
            ComputationDiagnosticStage::ProjectIpcResult,
            started.elapsed(),
        );
    })?;
    Ok(ExecutedComputation {
        outcome,
        reuse_digest,
    })
}

fn record_adapter_failure(
    core: &AppCore,
    stage: ComputationDiagnosticStage,
    elapsed: std::time::Duration,
) {
    let duration_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
    let _ = core.record_computation_stage_failure(stage, duration_ms);
    #[cfg(debug_assertions)]
    core.record_development_tool_trace(&serde_json::json!({
        "event": "computation_adapter_failed",
        "stage": stage.name(),
    }));
}

fn retention_timestamps(now: OffsetDateTime) -> Result<(String, String), IpcError> {
    let created = now
        .replace_nanosecond(0)
        .map_err(|_| IpcError::internal())?;
    let created_at = created.format(&Rfc3339).map_err(|_| IpcError::internal())?;
    let expires_at = (created + Duration::days(7))
        .format(&Rfc3339)
        .map_err(|_| IpcError::internal())?;
    Ok((created_at, expires_at))
}

pub(super) fn current_reuse_digest(
    core: &AppCore,
    conversation: &Conversation,
    workspace: Option<&ChatToolWorkspace>,
    call: &ParsedToolCall,
) -> Result<String, IpcError> {
    validate_call(call)?;
    let request_id = "reuse-check";
    let source = required_string(call, "source")?.to_owned();
    let output = output_expectation(call)?;
    if call.name == PURE_COMPUTATION_TOOL {
        let request = ComputationRequest::new(request_id, source, Vec::new(), output)
            .map_err(|_| IpcError::invalid_request("The model returned invalid tool arguments"))?;
        return computation_reuse_digest(&request, None).map_err(|_| IpcError::internal());
    }
    let workspace = workspace.ok_or_else(|| {
        IpcError::invalid_request("The requested project workspace is unavailable")
    })?;
    let snapshot = bind_snapshot(core, conversation, workspace, call)?;
    core.with_admitted_project_computation_inputs(&snapshot, |inputs| {
        ComputationRequest::new(request_id, source, inputs.to_vec(), output)
            .map_err(|_| IpcError::invalid_request("The model returned invalid tool arguments"))
            .and_then(|request| {
                computation_reuse_digest(&request, Some(&snapshot))
                    .map_err(|_| IpcError::internal())
            })
    })
    .map_err(app_core_error)?
}

fn bind_snapshot(
    core: &AppCore,
    conversation: &Conversation,
    workspace: &ChatToolWorkspace,
    call: &ParsedToolCall,
) -> Result<ProjectComputationInputSnapshot, IpcError> {
    let references = input_references(call)?;
    let bookmark = core
        .project_workspace_bookmark_data(&conversation.project_id, &workspace.grant_id)
        .map_err(app_core_error)?;
    let access = workspace_bookmark::access(&bookmark).map_err(bookmark_access_error)?;
    core.bind_project_computation_inputs(
        &conversation.project_id,
        &workspace.grant_id,
        access.path(),
        &references,
    )
    .map_err(app_core_error)
}

fn run(
    runner: &ComputationRunner,
    request: &ComputationRequest,
    cancellation: &AtomicBool,
) -> (
    agent_core::ComputationTerminalStatus,
    Option<computation_runtime::ComputationOutput>,
) {
    match runner.execute_with_host_cancellation(request, cancellation) {
        Ok(response) => (
            computation_terminal_status(response.status()),
            response.output().cloned(),
        ),
        Err(error) => (computation_runner_error_status(error), None),
    }
}

fn ipc_outcome(envelope: &ToolResultEnvelope) -> Result<ToolOutcome, IpcError> {
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
                                    JsonValue::String("compactToolResult".to_owned()),
                                ),
                            ])),
                        )])),
                    }),
                    ResultContent::ValidatedAsset { .. } => Err(IpcError::internal()),
                })
                .collect::<Result<_, _>>()?,
        },
        AgentToolOutcome::Failed { failure } => ToolOutcome::Failed {
            error: IpcError {
                code: IpcErrorCode::Unsupported,
                message: failure.message().to_owned(),
                retryable: failure.transient(),
            },
        },
        AgentToolOutcome::Cancelled { reason } => ToolOutcome::Failed {
            error: IpcError {
                code: IpcErrorCode::Cancelled,
                message: match reason {
                    CancellationReason::UserRequested => "The computation was stopped",
                    CancellationReason::DeadlineExceeded => "The computation exceeded its deadline",
                    CancellationReason::PermissionWithdrawn => "Project access was withdrawn",
                    CancellationReason::HostShutdown => "The application stopped the computation",
                }
                .to_owned(),
                retryable: false,
            },
        },
    })
}

fn required_string<'a>(call: &'a ParsedToolCall, key: &str) -> Result<&'a str, IpcError> {
    call.arguments
        .get(key)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| IpcError::invalid_request("The model returned invalid tool arguments"))
}

fn output_expectation(call: &ParsedToolCall) -> Result<OutputExpectation, IpcError> {
    let output = call
        .arguments
        .get("output")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| IpcError::invalid_request("The model returned invalid tool arguments"))?;
    let kind = match output.get("kind").and_then(serde_json::Value::as_str) {
        Some("text") => OutputKind::Text,
        Some("json") => OutputKind::Json,
        _ => {
            return Err(IpcError::invalid_request(
                "The model returned invalid tool arguments",
            ));
        }
    };
    let max_bytes = output
        .get("maxBytes")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| IpcError::invalid_request("The model returned invalid tool arguments"))?;
    let max_items = output
        .get("maxItems")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| IpcError::invalid_request("The model returned invalid tool arguments"))?;
    OutputExpectation::new(kind, max_bytes, max_items)
        .map_err(|_| IpcError::invalid_request("The model returned invalid tool arguments"))
}

fn input_references(
    call: &ParsedToolCall,
) -> Result<Vec<ProjectComputationInputReference>, IpcError> {
    call.arguments
        .get("inputs")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| IpcError::invalid_request("The model returned invalid tool arguments"))?
        .iter()
        .map(|value| {
            let value = value.as_object().ok_or_else(|| {
                IpcError::invalid_request("The model returned invalid tool arguments")
            })?;
            let format = match value.get("format").and_then(serde_json::Value::as_str) {
                Some("utf8") => InputFormat::Utf8,
                Some("json") => InputFormat::Json,
                Some("bytes") => InputFormat::Bytes,
                _ => {
                    return Err(IpcError::invalid_request(
                        "The model returned invalid tool arguments",
                    ));
                }
            };
            ProjectComputationInputReference::new(
                value
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default(),
                value
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default(),
                format,
            )
            .map_err(|_| IpcError::invalid_request("The model returned invalid tool arguments"))
        })
        .collect()
}

fn runner_executable() -> PathBuf {
    let parent = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(std::path::Path::to_path_buf))
        .unwrap_or_default();
    let bundle_dir = if parent.file_name().is_some_and(|name| name == "deps") {
        parent.parent().unwrap_or(&parent)
    } else {
        &parent
    };
    let packaged_macos_app = bundle_dir.file_name().is_some_and(|name| name == "MacOS")
        && bundle_dir
            .parent()
            .and_then(|path| path.file_name())
            .is_some_and(|name| name == "Contents");
    bundle_dir.join(if packaged_macos_app {
        "zeta-computation-broker"
    } else {
        "zeta-computation-runner"
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_timestamps_are_canonical_second_precision()
    -> Result<(), Box<dyn std::error::Error>> {
        let now = OffsetDateTime::parse("2026-08-22T16:14:24.845123Z", &Rfc3339)?;
        let (created_at, expires_at) = retention_timestamps(now)
            .map_err(|_| std::io::Error::other("retention timestamp formatting failed"))?;
        assert_eq!(created_at, "2026-08-22T16:14:24Z");
        assert_eq!(expires_at, "2026-08-29T16:14:24Z");
        assert_eq!(created_at.len(), 20);
        assert_eq!(expires_at.len(), 20);
        Ok(())
    }

    #[test]
    fn domain_validation_enforces_text_shape_after_closed_schema_validation() {
        let call = ParsedToolCall {
            call_id: "pure-computation".to_owned(),
            recipient: PURE_COMPUTATION_TOOL.to_owned(),
            name: PURE_COMPUTATION_TOOL.to_owned(),
            arguments: BTreeMap::from([
                (
                    "output".to_owned(),
                    serde_json::json!({"kind":"text","maxBytes":1024,"maxItems":2}),
                ),
                (
                    "source".to_owned(),
                    serde_json::json!("export default function compute() { return 'ok'; }"),
                ),
            ]),
        };
        assert!(validate_call(&call).is_err());
    }
}
