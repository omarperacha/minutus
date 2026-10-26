//! Bounded scheduling for one model-authored batch of chat tool calls.

use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

use app_core::AppCore;
use ipc_types::{
    Conversation, GenerationEvent, IpcError, IpcErrorCode, RequestId, TaskActivityStatus,
    ToolOutcome,
};
use model_runtime::ParsedToolCall;

use crate::chat_runtime::{emit_chat_event, next_sequence};

mod corrections;
mod plan_deferral;

pub(crate) use plan_deferral::{
    PLAN_PENDING_GUIDANCE, PLAN_REQUIRED_OPERATION, deferred_plan_exchanges, emit_plan_deferrals,
    plan_deferral_batch_is_bounded,
};

pub(crate) use corrections::{
    complete_control_call, inactive_tool_batch_repairable, invalid_tool_batch_repairable,
    mixed_read_only_invalid_batch_repairable, plan_prefixed_invalid_operation_batch_repairable,
    reject_final_response_tool_batch, reject_inactive_tool_batch, reject_invalid_tool_batch,
    reject_plan_deviation_batch, reject_unknown_tool_batch, unknown_tool_batch_repairable,
};
#[cfg(test)]
use corrections::{
    final_response_correction_exchange, plan_deviation_correction_exchange,
    tool_argument_correction_exchange, tool_availability_correction_exchange,
};

use super::{
    ChatToolExchange, ChatToolWorkspace, OperationLedger, PathReferenceLedger, computation,
    execution::{
        PreparedChatTool, PreparedToolCorrection, execute_prepared_chat_tool,
        inactive_tool_call_is_safely_bounded, prepare_chat_tool, prepare_inactive_tool_correction,
        prepare_invalid_tool_correction, prepare_unknown_tool_correction,
        unknown_tool_call_is_safely_bounded,
    },
    validation::chat_tool_contract,
};

const MAX_PARALLEL_READS: usize = 4;
const MAX_TRANSIENT_RETRIES: u32 = 2;
const RETRY_BACKOFF_MS: u64 = 25;

#[derive(Debug, Eq, PartialEq)]
struct BatchPlan {
    execution_order: Vec<usize>,
    duplicate_of: Vec<Option<usize>>,
    concurrent_reads: bool,
}

struct BatchEventSink<'a> {
    app: &'a tauri::AppHandle,
    request_id: &'a RequestId,
    sequence: &'a mut u32,
    conversation: &'a Conversation,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn execute_chat_tool_batch(
    app: &tauri::AppHandle,
    core: &AppCore,
    request_id: &RequestId,
    sequence: &mut u32,
    conversation: &Conversation,
    workspaces: &[ChatToolWorkspace],
    calls: &[ParsedToolCall],
    operation_ledger: &OperationLedger,
    path_references: &mut PathReferenceLedger,
    remaining_image_slots: &mut usize,
    cancellation_requested: &AtomicBool,
) -> Vec<Result<ChatToolExchange, IpcError>> {
    let prepared = calls
        .iter()
        .map(|call| prepare_chat_tool(workspaces, call))
        .collect::<Vec<_>>();
    if prepared.iter().any(Result::is_err) {
        return prepared
            .into_iter()
            .map(|item| item.and_then(|_| Err(IpcError::internal())))
            .collect();
    }
    let prepared = prepared
        .into_iter()
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    let mut events = BatchEventSink {
        app,
        request_id,
        sequence,
        conversation,
    };
    for tool in &prepared {
        emit_start(&mut events, tool);
    }

    let Ok(limits) = core.tool_schedule_limits(conversation, &request_id.0) else {
        return prepared.iter().map(|_| Err(IpcError::internal())).collect();
    };
    let mut plan = plan_batch(&prepared);
    plan.execution_order = admit_execution_order(
        &plan.execution_order,
        &prepared,
        limits.max_calls(),
        limits.max_output_bytes(),
    );
    let results = execute_plan(
        app,
        core,
        conversation,
        &prepared,
        &plan,
        operation_ledger,
        remaining_image_slots,
        cancellation_requested,
        Duration::from_millis(limits.max_elapsed_ms()),
    );
    retain_bounded_results(
        &mut events,
        &prepared,
        results,
        path_references,
        limits.max_output_bytes(),
        cancellation_requested,
    )
}

#[allow(clippy::too_many_arguments)]
fn execute_plan(
    app: &tauri::AppHandle,
    core: &AppCore,
    conversation: &Conversation,
    prepared: &[PreparedChatTool],
    plan: &BatchPlan,
    operation_ledger: &OperationLedger,
    remaining_image_slots: &mut usize,
    cancellation_requested: &AtomicBool,
    max_elapsed: Duration,
) -> Vec<Option<Result<ChatToolExchange, IpcError>>> {
    let started = Instant::now();
    let mut results = prepared
        .iter()
        .map(|tool| {
            if tool.call.name == "x402.request" && operation_ledger.x402_attempt_consumed() {
                return Some(Ok(x402_attempt_consumed_exchange(tool)));
            }
            let digest = operation_ledger
                .has_computation_candidate(tool)
                .then(|| {
                    let workspace = tool.workspace.as_ref();
                    computation::current_reuse_digest(core, conversation, workspace, &tool.call)
                })
                .transpose()
                .ok()
                .flatten();
            operation_ledger
                .reusable_exchange(tool, digest.as_deref())
                .map(Ok)
        })
        .collect::<Vec<_>>();
    if plan.concurrent_reads {
        for wave in plan.execution_order.chunks(MAX_PARALLEL_READS) {
            if should_stop(cancellation_requested, started, max_elapsed) {
                break;
            }
            thread::scope(|scope| {
                let handles = wave
                    .iter()
                    .filter(|index| results[**index].is_none())
                    .map(|index| {
                        let tool = prepared[*index].clone();
                        (
                            *index,
                            scope.spawn(move || {
                                execute_with_retry(
                                    app,
                                    core,
                                    conversation,
                                    &tool,
                                    true,
                                    cancellation_requested,
                                )
                            }),
                        )
                    })
                    .collect::<Vec<_>>();
                for (index, handle) in handles {
                    results[index] =
                        Some(handle.join().unwrap_or_else(|_| Err(IpcError::internal())));
                }
            });
        }
    } else {
        for index in &plan.execution_order {
            if results[*index].is_some() {
                continue;
            }
            if should_stop(cancellation_requested, started, max_elapsed) {
                break;
            }
            let image_slot_available = *remaining_image_slots > 0;
            let result = execute_with_retry(
                app,
                core,
                conversation,
                &prepared[*index],
                image_slot_available,
                cancellation_requested,
            );
            if result
                .as_ref()
                .ok()
                .and_then(|exchange| exchange.inspected_asset_id.as_ref())
                .is_some()
            {
                *remaining_image_slots = remaining_image_slots.saturating_sub(1);
            }
            results[*index] = Some(result);
        }
    }

    for (index, source) in plan.duplicate_of.iter().enumerate() {
        if let Some(source) = source
            && results[*source].is_some()
        {
            results[index] = Some(reuse_exchange(results[*source].as_ref(), &prepared[index]));
        }
    }
    results
}

fn retain_bounded_results(
    events: &mut BatchEventSink<'_>,
    prepared: &[PreparedChatTool],
    results: Vec<Option<Result<ChatToolExchange, IpcError>>>,
    path_references: &mut PathReferenceLedger,
    max_output_bytes: usize,
    cancellation_requested: &AtomicBool,
) -> Vec<Result<ChatToolExchange, IpcError>> {
    let mut retained_bytes = 0_usize;
    results
        .into_iter()
        .enumerate()
        .map(|(index, result)| {
            let result = retain_scheduled_result(
                &prepared[index],
                result,
                path_references,
                &mut retained_bytes,
                max_output_bytes,
                cancellation_requested.load(Ordering::Acquire),
            );
            if let Ok(exchange) = &result {
                emit_finish(events, exchange);
            }
            result
        })
        .collect()
}

fn retain_scheduled_result(
    prepared: &PreparedChatTool,
    result: Option<Result<ChatToolExchange, IpcError>>,
    path_references: &mut PathReferenceLedger,
    retained_bytes: &mut usize,
    max_output_bytes: usize,
    cancelled: bool,
) -> Result<ChatToolExchange, IpcError> {
    let mut exchange = match result {
        Some(result) => result?,
        None if cancelled => return Ok(cancelled_exchange(prepared, "Cancelled before execution")),
        None => {
            return Ok(partial_exchange(
                prepared,
                ScheduledReadDisposition::Deferred,
            ));
        }
    };
    // Admission/cancellation diagnostics are already compact. Never rewrite a
    // non-executed result as though its read had run when the budget is full.
    if exchange.activity.status == TaskActivityStatus::Partial {
        return Ok(exchange);
    }
    let mut candidate = path_references.clone();
    candidate.annotate_exchange(&mut exchange);
    let bytes = serde_json::to_vec(&exchange.result).map_or(usize::MAX, |encoded| encoded.len());
    if is_concurrent_read(&prepared.call) && retained_bytes.saturating_add(bytes) > max_output_bytes
    {
        return Ok(partial_exchange(
            prepared,
            ScheduledReadDisposition::OutputWithheld,
        ));
    }
    *retained_bytes = retained_bytes.saturating_add(bytes);
    *path_references = candidate;
    Ok(exchange)
}

fn plan_batch(prepared: &[PreparedChatTool]) -> BatchPlan {
    let concurrent_reads = prepared.iter().all(|tool| is_concurrent_read(&tool.call));
    let mut first_by_key = BTreeMap::new();
    let mut first_x402 = None;
    let mut duplicate_of = vec![None; prepared.len()];
    let mut execution_order = Vec::new();
    for (index, tool) in prepared.iter().enumerate() {
        let key = equivalence_key(&tool.call);
        if tool.call.name == "x402.request" {
            if let Some(first) = first_x402 {
                duplicate_of[index] = Some(first);
            } else {
                first_x402 = Some(index);
                execution_order.push(index);
            }
        } else if matches!(
            tool.call.name.as_str(),
            "computation.run" | "computation.run_project" | "web.retrieve" | "web.search"
        ) {
            execution_order.push(index);
        } else if let Some(first) = first_by_key.get(&key) {
            duplicate_of[index] = Some(*first);
        } else {
            first_by_key.insert(key, index);
            execution_order.push(index);
        }
    }
    execution_order.sort_by_key(|index| (aggregate_rank(&prepared[*index].call), *index));
    BatchPlan {
        execution_order,
        duplicate_of,
        concurrent_reads,
    }
}

fn admit_execution_order(
    order: &[usize],
    prepared: &[PreparedChatTool],
    max_calls: usize,
    max_output_bytes: usize,
) -> Vec<usize> {
    let mut output_bytes = 0_usize;
    order
        .iter()
        .copied()
        .filter(|index| {
            let estimate = estimated_output_bytes(&prepared[*index].call);
            let admitted = output_bytes
                .checked_add(estimate)
                .is_some_and(|total| total <= max_output_bytes);
            if admitted {
                output_bytes = output_bytes.saturating_add(estimate);
            }
            admitted
        })
        .take(max_calls)
        .collect()
}

fn execute_with_retry(
    app: &tauri::AppHandle,
    core: &AppCore,
    conversation: &Conversation,
    prepared: &PreparedChatTool,
    image_slot_available: bool,
    cancellation_requested: &AtomicBool,
) -> Result<ChatToolExchange, IpcError> {
    let may_retry = is_concurrent_read(&prepared.call);
    for attempt in 0..=MAX_TRANSIENT_RETRIES {
        if cancellation_requested.load(Ordering::Acquire) {
            return Ok(cancelled_exchange(
                prepared,
                "Cancelled during tool execution",
            ));
        }
        let result = Ok(execute_prepared_chat_tool(
            app,
            core,
            conversation,
            prepared.clone(),
            image_slot_available,
            cancellation_requested,
        ));
        let transient = result.as_ref().ok().is_some_and(|exchange| {
            matches!(
                &exchange.result.outcome,
                ToolOutcome::Failed { error }
                    if error.code == IpcErrorCode::Internal && error.retryable
            )
        });
        if !may_retry || !transient || attempt == MAX_TRANSIENT_RETRIES {
            return result;
        }
        let delay = RETRY_BACKOFF_MS.saturating_mul(1_u64 << attempt);
        thread::sleep(Duration::from_millis(delay));
    }
    Err(IpcError::internal())
}

fn reuse_exchange(
    source: Option<&Result<ChatToolExchange, IpcError>>,
    target: &PreparedChatTool,
) -> Result<ChatToolExchange, IpcError> {
    if target.call.name == "x402.request" {
        return Ok(x402_attempt_consumed_exchange(target));
    }
    let mut exchange = source
        .and_then(|source| source.as_ref().ok())
        .cloned()
        .ok_or_else(IpcError::internal)?;
    exchange.request = target.request.clone();
    exchange.result.call_id = target.request.call_id.clone();
    exchange.activity = target.activity.clone();
    exchange.activity.status = TaskActivityStatus::Reused;
    exchange.activity.detail = Some("Reused an equivalent result from this read batch".to_owned());
    exchange.undo_changes.clear();
    exchange.inspected_asset_id = None;
    Ok(exchange)
}

fn x402_attempt_consumed_exchange(target: &PreparedChatTool) -> ChatToolExchange {
    failed_exchange(
        target,
        IpcError {
            code: IpcErrorCode::InvalidRequest,
            message: "An x402 payment workflow was already attempted for this user request; no second authorization or payment attempt was started".to_owned(),
            retryable: false,
        },
        TaskActivityStatus::Failed,
        "Request-local x402 payment attempt already consumed",
    )
}

fn cancelled_exchange(target: &PreparedChatTool, detail: &str) -> ChatToolExchange {
    failed_exchange(
        target,
        IpcError {
            code: IpcErrorCode::Cancelled,
            message: "The tool operation was cancelled".to_owned(),
            retryable: false,
        },
        TaskActivityStatus::Partial,
        detail,
    )
}

/// Scheduler outcomes are not deterministic operation failures. Keep the two
/// cases distinct even though neither retained result can enter the reuse ledger.
#[derive(Clone, Copy)]
enum ScheduledReadDisposition {
    Deferred,
    OutputWithheld,
}

fn partial_exchange(
    target: &PreparedChatTool,
    disposition: ScheduledReadDisposition,
) -> ChatToolExchange {
    let detail = match disposition {
        ScheduledReadDisposition::Deferred => {
            "Not executed because this batch reached its scheduling budget. Retry this operation separately in a later batch within the remaining request budget."
        }
        ScheduledReadDisposition::OutputWithheld => {
            "The read was executed, but its result was withheld because this batch reached its result budget. Retry this read separately with a smaller output limit where supported. No write was repeated."
        }
    };
    failed_exchange(
        target,
        IpcError {
            code: IpcErrorCode::PayloadTooLarge,
            message: detail.to_owned(),
            retryable: true,
        },
        TaskActivityStatus::Partial,
        detail,
    )
}

fn failed_exchange(
    target: &PreparedChatTool,
    error: IpcError,
    status: TaskActivityStatus,
    detail: &str,
) -> ChatToolExchange {
    ChatToolExchange {
        request: target.request.clone(),
        result: ipc_types::ToolResult {
            call_id: target.request.call_id.clone(),
            outcome: ToolOutcome::Failed { error },
        },
        activity: ipc_types::TaskActivity {
            status,
            detail: Some(detail.to_owned()),
            ..target.activity.clone()
        },
        workspace_grant_id: target
            .workspace
            .as_ref()
            .map(|workspace| workspace.grant_id.clone()),
        undo_changes: Vec::new(),
        inspected_asset_id: None,
        computation_reuse_digest: None,
    }
}

fn emit_start(events: &mut BatchEventSink<'_>, tool: &PreparedChatTool) {
    emit_request_start(events, &tool.request, &tool.activity);
}

fn emit_correction_start(events: &mut BatchEventSink<'_>, tool: &PreparedToolCorrection) {
    emit_request_start(events, &tool.request, &tool.activity);
}

fn emit_request_start(
    events: &mut BatchEventSink<'_>,
    request: &ipc_types::ToolRequest,
    activity: &ipc_types::TaskActivity,
) {
    emit_chat_event(
        events.app,
        &events.conversation.id,
        GenerationEvent::ToolCall {
            request_id: events.request_id.clone(),
            sequence: next_sequence(events.sequence),
            request: request.clone(),
        },
    );
    emit_chat_event(
        events.app,
        &events.conversation.id,
        GenerationEvent::Activity {
            request_id: events.request_id.clone(),
            sequence: next_sequence(events.sequence),
            activity: activity.clone(),
        },
    );
}

fn emit_finish(events: &mut BatchEventSink<'_>, exchange: &ChatToolExchange) {
    emit_chat_event(
        events.app,
        &events.conversation.id,
        GenerationEvent::Activity {
            request_id: events.request_id.clone(),
            sequence: next_sequence(events.sequence),
            activity: exchange.activity.clone(),
        },
    );
}

fn should_stop(
    cancellation_requested: &AtomicBool,
    started: Instant,
    max_elapsed: Duration,
) -> bool {
    cancellation_requested.load(Ordering::Acquire) || started.elapsed() >= max_elapsed
}

fn equivalence_key(call: &ParsedToolCall) -> String {
    serde_json::to_string(&(&call.name, &call.arguments)).unwrap_or_default()
}

fn is_concurrent_read(call: &ParsedToolCall) -> bool {
    matches!(
        call.name.as_str(),
        "filesystem.list"
            | "filesystem.find_paths"
            | "filesystem.stat"
            | "filesystem.read_text"
            | "filesystem.search"
            | "filesystem.scan"
    )
}

fn aggregate_rank(call: &ParsedToolCall) -> u8 {
    match call.name.as_str() {
        "filesystem.collect_evidence"
        | "filesystem.find_paths"
        | "filesystem.scan"
        | "filesystem.search" => 0,
        _ => 1,
    }
}

fn estimated_output_bytes(call: &ParsedToolCall) -> usize {
    match call.name.as_str() {
        "filesystem.stat" | "filesystem.inspect_image" => 4 * 1024,
        "filesystem.list" | "filesystem.find_paths" | "filesystem.scan" => 128 * 1024,
        "filesystem.search" => 512 * 1024,
        "filesystem.collect_evidence"
        | "filesystem.read_text"
        | "filesystem.create_directory"
        | "filesystem.write_text"
        | "filesystem.replace_text"
        | "filesystem.write_delimited_table"
        | "orchestration.transform_artifact" => 1024 * 1024,
        "computation.run" | "computation.run_project" | "web.retrieve" | "x402.request" => {
            64 * 1024
        }
        "web.search" => 32 * 1024,
        _ => usize::MAX,
    }
}

#[cfg(test)]
mod tests;
