//! Bounded inference rounds and terminal outcome handling.

#[cfg(debug_assertions)]
use std::time::Duration;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use agent_core::{
    ExecutionComplexity, ExecutionProfile, ExecutionProfileDecision, ExecutionProfileSignals,
    ExecutionRisk, ExecutionWorkKind,
};
use app_core::{
    AppCore, AppCoreError, GenerationFailureCategory, PreparedChatGeneration,
    RichInputPresentationPolicy, ToolExecutionState, ToolOperationBudget, ToolOrchestrationContext,
    ToolOrchestrationPhase, ToolPhaseScope,
};
use ipc_types::{
    ChatEffort, ChatGenerationPhase, Conversation, FinishReason, GenerationEvent, IpcError,
    IpcErrorCode, MAX_CHAT_ASSETS, MessagePart, MessageRole, RequestId, TaskActivity,
    TaskActivityKind, TaskActivityStatus, ToolOutcome,
};
#[cfg(any(debug_assertions, test))]
use model_runtime::PromptAdapterFailureKind;
use model_runtime::{
    GenerationCompletion, GenerationEvent as ModelGenerationEvent, GenerationMetrics,
    InferenceError, ModelOutputFailureKind, ParsedToolCall, PromptToolDefinition,
};
use sha2::{Digest as _, Sha256};
use time::OffsetDateTime;

mod budget;
#[cfg(test)]
mod direct_transfer_tests;
mod dispatch;
mod images;
mod plan_pending;
mod recall_scope;
mod recovery_flow;
mod round;
mod state;
mod stream_display;
mod telemetry;
mod workspace_access;

use dispatch::{
    bounded_stage_asks_user, bounded_stage_tools, execute_forecast_prefixed_round,
    execute_plan_prefixed_round, execute_positioned_stage_completion_round,
    execute_tool_search_round, execute_workspace_tool_round, is_forecast_call, is_plan_call,
    is_plan_relationship_error, is_stage_completion_call, is_tool_search_call,
    should_schedule_forecast_recovery, should_schedule_outcome_recovery,
    terminal_without_tool_calls, uses_dedicated_forecast_dispatch,
    uses_dedicated_stage_completion_dispatch, uses_dedicated_tool_search_dispatch,
};
use recovery_flow::{
    argument_correction_fingerprint, completion_reason_name,
    exhausted_provider_recovery_with_cause, file_content_mutation_completed,
    record_argument_correction, tool_call_fingerprint, tool_preparation_terminal_error,
    validate_or_recover_phase_calls,
};
use round::{execute_chat_generation_round, format_model_pass_duration, model_pass_activity};
use state::{
    GenerationToolState, ProviderPromptPrefixBoundary, ProviderPromptPrefixTracker,
    output_exhaustion_recovery_diagnostic,
};
use telemetry::{
    ChatGenerationRound, ModelStreamProgress, RequestExecutionTelemetry, RoundAdmissionTelemetry,
    applied_change_confirmation, applied_change_incomplete_error, calls_available_in_phase,
    elapsed_ms, next_consecutive_discovery_calls, remaining_plan_forecast_calls,
    resolved_stream_failure, terminal_after_mutation, tool_capabilities, trace_emitted_tool_calls,
    trace_execution_forecast, trace_model_round_completed, trace_model_round_failed,
    trace_model_round_preparation_started, trace_model_round_provider_requested,
    trace_model_round_started, trace_model_stream_rejection, trace_plan_outcome_recovery,
    trace_plan_repair_decision, trace_plan_tool_results, trace_response_persistence,
    trace_tool_exchanges, trace_tool_rejection,
};
#[cfg(debug_assertions)]
use telemetry::{RichInputAdmission, RoundTiming, ns_to_ms};

use crate::{
    app_state::{ChatGenerationControl, MAX_CHAT_TOOL_CALLS, MAX_CHAT_TOOL_ROUNDS},
    chat_tools::{
        ChatToolExchange, OperationLedger, PathReferenceLedger, SEARCH_TOOLS_TOOL, ToolActionClass,
        applied_changes_verified, applied_paths, chat_tool_definitions, chat_tool_workspaces,
        complete_control_call, conversation_image_count, exact_argument_repair_error,
        execute_chat_tool_batch, inactive_tool_batch_repairable, invalid_tool_batch_repairable,
        mixed_read_only_invalid_batch_repairable, normalize_artifact_arguments,
        persist_tool_exchanges, plan_prefixed_invalid_operation_batch_repairable,
        reject_final_response_tool_batch, reject_inactive_tool_batch, reject_invalid_tool_batch,
        reject_plan_deviation_batch, reject_unknown_tool_batch, search_tool_definitions,
        tool_action_class, tool_batch_completed, tool_category_index,
        unknown_tool_batch_repairable, validated_chat_tool_request,
    },
    error_mapping::{current_timestamp, generation_error, stream_error},
};

#[cfg(debug_assertions)]
use super::plan::ForecastDiagnosticMatch;
use super::{
    events::{emit_chat_event, emit_chat_phase, next_sequence, tool_loop_limit_error},
    plan::{
        BoundedStageContract, COMPLETE_STAGE_TOOL, FORECAST_TOOL, PlanPolicy, plan_repair_defects,
    },
    recovery::{
        MAX_RECOVERY_ATTEMPTS, PlanDefectProgress, RecoveryCheckpoint, RecoveryCoordinator,
        RecoveryDecision, RecoveryKind,
    },
    rich_input::RichInputPresentationLedger,
};

const MAX_ARGUMENT_CORRECTIONS: usize = 6;
#[cfg(debug_assertions)]
const STREAM_PROGRESS_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct ChatGenerationSettings {
    pub(super) context_tokens: Option<u32>,
    pub(super) effort: Option<ChatEffort>,
}

impl ChatGenerationSettings {
    fn apply_effort(self, mut decision: ExecutionProfileDecision) -> ExecutionProfileDecision {
        if let Some(effort) = self.effort {
            decision.profile = match effort {
                ChatEffort::Quick => ExecutionProfile::Latency,
                ChatEffort::Balanced => ExecutionProfile::Balanced,
                ChatEffort::Deep => ExecutionProfile::Deep,
            };
        }
        decision
    }
}

#[allow(clippy::needless_pass_by_value)]
#[expect(
    clippy::too_many_arguments,
    reason = "the worker binds one request's trusted runtime settings to its conversation and cancellation state"
)]
pub(super) fn run_chat_generation(
    app: tauri::AppHandle,
    core: Arc<AppCore>,
    control: Arc<ChatGenerationControl>,
    executable: PathBuf,
    request_id: RequestId,
    conversation: Conversation,
    cancellation_requested: Arc<AtomicBool>,
    settings: ChatGenerationSettings,
) {
    let conversation_id = conversation.id.clone();
    let mut sequence = 0_u32;
    emit_chat_phase(
        &app,
        &conversation_id,
        &request_id,
        &mut sequence,
        ChatGenerationPhase::StartingResponse,
    );
    let mut model_passes = Vec::new();
    let terminal = execute_chat_generation(
        &app,
        &core,
        &executable,
        &request_id,
        &conversation,
        &cancellation_requested,
        &mut sequence,
        &mut model_passes,
        settings,
    );
    let event = match terminal {
        ChatTerminal::Final {
            reasoning,
            text,
            reason,
            mutation_applied,
        } => {
            emit_chat_phase(
                &app,
                &conversation_id,
                &request_id,
                &mut sequence,
                ChatGenerationPhase::SavingResponse,
            );
            let persistence_started = Instant::now();
            let persisted = current_timestamp().and_then(|completed_at| {
                core.append_assistant_message(
                    &conversation_id,
                    reasoning,
                    model_passes,
                    text.clone(),
                    &completed_at,
                )
                .map_err(generation_error)
            });
            trace_response_persistence(
                &core,
                &request_id,
                elapsed_ms(persistence_started),
                persisted.is_ok(),
            );
            match persisted {
                Ok(message) => GenerationEvent::Final {
                    request_id: request_id.clone(),
                    sequence: next_sequence(&mut sequence),
                    message_id: message.id,
                    text,
                    finish_reason: reason,
                },
                Err(error) => {
                    let _ = core.record_generation_failure(GenerationFailureCategory::Persistence);
                    GenerationEvent::Error {
                        request_id: Some(request_id.clone()),
                        sequence: next_sequence(&mut sequence),
                        error: if mutation_applied {
                            applied_change_incomplete_error()
                        } else {
                            error
                        },
                    }
                }
            }
        }
        ChatTerminal::Cancelled => GenerationEvent::Cancelled {
            request_id: request_id.clone(),
            sequence: next_sequence(&mut sequence),
        },
        ChatTerminal::Error { error, category } => {
            let _ = core.record_generation_failure(category);
            GenerationEvent::Error {
                request_id: Some(request_id.clone()),
                sequence: next_sequence(&mut sequence),
                error,
            }
        }
    };
    emit_chat_event(&app, &conversation_id, event);
    control.finish(&request_id);
}

pub(super) enum ChatTerminal {
    Final {
        reasoning: Option<String>,
        text: String,
        reason: FinishReason,
        mutation_applied: bool,
    },
    Cancelled,
    Error {
        error: IpcError,
        category: GenerationFailureCategory,
    },
}

fn terminal_error(error: IpcError, category: GenerationFailureCategory) -> ChatTerminal {
    ChatTerminal::Error { error, category }
}

pub(super) struct ReasoningEchoFilter {
    prompt: Option<String>,
    pending: String,
    decided: bool,
}

impl ReasoningEchoFilter {
    fn new(prompt: Option<&str>) -> Self {
        Self {
            prompt: prompt.map(str::to_owned),
            pending: String::new(),
            decided: prompt.is_none(),
        }
    }

    fn push(&mut self, text: &str) -> Option<String> {
        if self.decided {
            return (!text.is_empty()).then(|| text.to_owned());
        }
        self.pending.push_str(text);
        let prompt = self.prompt.as_deref()?;
        if self.pending == prompt {
            return None;
        }
        if self.pending.starts_with(prompt) {
            let remainder = self.pending[prompt.len()..]
                .trim_start_matches(char::is_whitespace)
                .to_owned();
            self.pending.clear();
            self.decided = true;
            return (!remainder.is_empty()).then_some(remainder);
        }
        if prompt.starts_with(&self.pending) {
            return None;
        }
        self.decided = true;
        Some(std::mem::take(&mut self.pending))
    }
}

pub(super) fn latest_user_text(conversation: &Conversation) -> Option<&str> {
    conversation
        .messages
        .iter()
        .rev()
        .find(|message| message.role == MessageRole::User)
        .and_then(|message| {
            message.parts.iter().find_map(|part| match part {
                MessagePart::Text { text } => Some(text.as_str()),
                _ => None,
            })
        })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn execute_chat_generation(
    app: &tauri::AppHandle,
    core: &AppCore,
    executable: &Path,
    request_id: &RequestId,
    conversation: &Conversation,
    cancellation_requested: &Arc<AtomicBool>,
    sequence: &mut u32,
    model_passes: &mut Vec<TaskActivity>,
    settings: ChatGenerationSettings,
) -> ChatTerminal {
    let mut telemetry = RequestExecutionTelemetry::new();
    let mut tool_state = GenerationToolState {
        recall_scope: recall_scope::RecallScope::for_conversation(conversation),
        ..GenerationToolState::default()
    };
    let terminal = execute_chat_generation_inner(
        app,
        core,
        executable,
        request_id,
        conversation,
        cancellation_requested,
        sequence,
        model_passes,
        &mut telemetry,
        &mut tool_state,
        settings,
    );
    let terminal = plan_pending::annotate_terminal(terminal, tool_state.plan_pending.as_ref());
    telemetry.finish(core, request_id, conversation, &terminal);
    terminal
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
fn execute_chat_generation_inner(
    app: &tauri::AppHandle,
    core: &AppCore,
    executable: &Path,
    request_id: &RequestId,
    conversation: &Conversation,
    cancellation_requested: &Arc<AtomicBool>,
    sequence: &mut u32,
    model_passes: &mut Vec<TaskActivity>,
    telemetry: &mut RequestExecutionTelemetry,
    tool_state: &mut GenerationToolState,
    settings: ChatGenerationSettings,
) -> ChatTerminal {
    let workspaces = match chat_tool_workspaces(core, &conversation.project_id) {
        Ok(workspaces) => workspaces,
        Err(error) => {
            return terminal_error(error, GenerationFailureCategory::ToolValidation);
        }
    };
    tool_state.path_references = PathReferenceLedger::for_conversation(conversation, &workspaces);
    let all_tools = match chat_tool_definitions(&workspaces) {
        Ok(tools) => tools,
        Err(error) => return terminal_error(error, GenerationFailureCategory::ToolValidation),
    };
    let (workspace_mutation_tools_available, suggested_execution_reserve) =
        tool_capabilities(&all_tools);
    let mut conversation = conversation.clone();
    let mut provider_prefix = ProviderPromptPrefixTracker::default();
    for round in 0..=MAX_CHAT_TOOL_ROUNDS {
        if cancellation_requested.load(Ordering::Acquire) {
            return ChatTerminal::Cancelled;
        }
        if let Err(terminal) = workspace_access::require_planned_write_access(
            tool_state,
            workspace_mutation_tools_available,
        ) {
            return terminal;
        }
        tool_state
            .operation_ledger
            .set_semantic_stage(tool_state.plan_policy.current_stage_id());
        tool_state.observe_recovery_checkpoint();
        tool_state.select_bounded_stage_contract(&all_tools, workspace_mutation_tools_available);
        let projected_tools = tool_state.projected_tools(&all_tools);
        let tools = if tool_state.plan_policy.uses_advisory_stages() {
            // V6 keeps one stable, permissive execution catalogue. The bounded
            // stage remains host-validated but narrows guidance, not exposure.
            projected_tools
        } else {
            bounded_stage_tools(projected_tools, tool_state.active_stage_contract.as_ref())
        };
        if let Err(terminal) = tool_state.admit_stable_execution_prefix(&tools) {
            return terminal;
        }
        let provisional_profile = tool_state.execution_profile(&conversation, 0, false);
        let provisional_profile = if tool_state.active_stage_contract.is_some() {
            provisional_profile.for_bounded_action()
        } else {
            provisional_profile
        };
        let provisional_profile = settings.apply_effort(provisional_profile);
        let provisional_orchestration = tool_state
            .context(
                round,
                suggested_execution_reserve,
                workspace_mutation_tools_available,
                provisional_profile,
                &all_tools,
                &tools,
            )
            .with_active_task_references(crate::chat_tools::active_task_references(&conversation));
        let presentation = tool_state.image_presentation_policy();
        let projected_image_ids = match core.projected_chat_image_ids(
            &conversation,
            &provisional_orchestration,
            &presentation,
        ) {
            Ok(image_ids) => image_ids,
            Err(error) => {
                return terminal_error(
                    generation_error(error),
                    GenerationFailureCategory::PromptPreparation,
                );
            }
        };
        let image_count = projected_image_ids.len();
        let reinspection = match tool_state.image_reinspection(core, &projected_image_ids) {
            Ok(reinspection) => reinspection,
            Err(error) => {
                return terminal_error(
                    generation_error(error),
                    GenerationFailureCategory::PromptPreparation,
                );
            }
        };
        let mut execution_profile =
            tool_state.execution_profile(&conversation, image_count, reinspection);
        if tool_state.active_stage_contract.is_some() {
            execution_profile = execution_profile.for_bounded_action();
        }
        execution_profile = settings.apply_effort(execution_profile);
        execution_profile.bulk_arguments = tools
            .iter()
            .any(|tool| crate::chat_tools::tool_has_bulk_arguments(&tool.name));
        if let Err(terminal) =
            tool_state.admit_inference(&conversation, &tools, image_count, execution_profile)
        {
            return terminal;
        }
        let orchestration = if execution_profile == provisional_profile {
            provisional_orchestration
        } else {
            tool_state
                .context(
                    round,
                    suggested_execution_reserve,
                    workspace_mutation_tools_available,
                    execution_profile,
                    &all_tools,
                    &tools,
                )
                .with_active_task_references(crate::chat_tools::active_task_references(
                    &conversation,
                ))
        };
        let generation_started = Instant::now();
        let mut generated = match execute_chat_generation_round(
            app,
            core,
            executable,
            request_id,
            &conversation,
            tools.clone(),
            &presentation,
            cancellation_requested,
            sequence,
            model_passes,
            round,
            orchestration,
            execution_profile,
            reinspection,
            tool_state.provider_prefix_boundary(),
            &mut provider_prefix,
            tool_state,
            settings,
        ) {
            Ok(generated) => generated,
            Err(terminal) => {
                let status = if matches!(terminal, ChatTerminal::Cancelled) {
                    TaskActivityStatus::Cancelled
                } else {
                    TaskActivityStatus::Failed
                };
                let duration = format_model_pass_duration(elapsed_ms(generation_started));
                let detail = if status == TaskActivityStatus::Cancelled {
                    format!("Stopped after {duration}")
                } else {
                    format!("Interrupted after {duration}")
                };
                let activity = model_pass_activity(round, status, detail);
                emit_chat_event(
                    app,
                    &conversation.id,
                    GenerationEvent::Activity {
                        request_id: request_id.clone(),
                        sequence: next_sequence(sequence),
                        activity: activity.clone(),
                    },
                );
                model_passes.push(activity);
                telemetry.record_failed_round(
                    core,
                    request_id,
                    &conversation,
                    round,
                    elapsed_ms(generation_started),
                    &terminal,
                );
                let recoverable = match &terminal {
                    ChatTerminal::Error { error, category }
                        if error.retryable
                            && matches!(
                                category,
                                GenerationFailureCategory::Inference
                                    | GenerationFailureCategory::PromptPreparation
                            ) =>
                    {
                        Some(error.clone())
                    }
                    _ => None,
                };
                let Some(error) = recoverable else {
                    return terminal;
                };
                if round >= MAX_CHAT_TOOL_ROUNDS {
                    return terminal;
                }
                if let Err(exhausted) = tool_state.request_transient_recovery(
                    RecoveryKind::TransientProvider,
                    tool_state.plan_policy.correction_scope(),
                    format!("provider:{:?}", error.code),
                    format!(
                        "The model/provider round failed transiently: {} Resume the same semantic step from the retained checkpoint; do not reconstruct prior evidence.",
                        error.message
                    ),
                ) {
                    return exhausted_provider_recovery_with_cause(exhausted, &error);
                }
                continue;
            }
        };
        if !generated.calls.is_empty() {
            emit_chat_phase(
                app,
                &conversation.id,
                request_id,
                sequence,
                ChatGenerationPhase::ExecutingTools,
            );
        }
        telemetry.record_round(&generated);
        telemetry.record_image_counts(tool_state.rich_input_presentations.counts());
        telemetry.set_plan_revision(tool_state.plan_policy.revision());
        let post_round_started = Instant::now();
        match plan_pending::defer_preplan_calls(
            app,
            core,
            request_id,
            sequence,
            &mut conversation,
            tool_state,
            &mut generated,
            round,
            cancellation_requested,
        ) {
            Ok(true) => {
                telemetry.finish_round_post(
                    core,
                    request_id,
                    &conversation,
                    round,
                    post_round_started,
                    true,
                    true,
                );
                continue;
            }
            Ok(false) => {}
            Err(terminal) => return terminal,
        }
        if should_schedule_forecast_recovery(
            generated.calls.is_empty(),
            tool_state.plan_policy.forecast_required(),
            round,
        ) {
            if let Err(terminal) = tool_state.request_recovery(
                RecoveryKind::ForecastValidation,
                tool_state.plan_policy.forecast_scope(),
                format!(
                    "missing_forecast:{}",
                    completion_reason_name(generated.reason)
                ),
                "The accepted legacy plan still requires orchestration.submit_forecast. Emit that structured call now; do not report task completion or restart planning.",
            ) {
                return terminal;
            }
            telemetry.finish_round_post(
                core,
                request_id,
                &conversation,
                round,
                post_round_started,
                false,
                true,
            );
            continue;
        }
        if !tool_state.plan_policy.forecast_required()
            && !bounded_stage_asks_user(&generated, tool_state.active_stage_contract.as_ref())
            && should_schedule_outcome_recovery(
                generated.calls.is_empty(),
                tool_state.plan_policy.requires_outcome_recovery(),
                round,
            )
        {
            let recovery_attempt = tool_state.outcome_recovery_attempts.saturating_add(1);
            let recovery_kind = if generated.reason == GenerationCompletion::Length {
                RecoveryKind::OutputExhaustion
            } else {
                RecoveryKind::PlanValidation
            };
            if let Err(terminal) = tool_state.request_recovery(
                recovery_kind,
                tool_state.plan_policy.correction_scope(),
                format!(
                    "execution_mode:{}:{recovery_attempt}",
                    completion_reason_name(generated.reason)
                ),
                if generated.reason == GenerationCompletion::Length {
                    output_exhaustion_recovery_diagnostic(&tool_state.plan_policy)
                } else {
                    "The prior round stopped without the operation required by the accepted outcome. Resume this same step and emit the outstanding call or identify one concrete blocker."
                },
            ) {
                return terminal;
            }
            tool_state.begin_outcome_recovery(generated.reason, &generated.image_ids);
            trace_plan_outcome_recovery(
                core,
                request_id,
                &conversation,
                round,
                generated.reason,
                MAX_CHAT_TOOL_CALLS.saturating_sub(tool_state.total_calls),
                MAX_CHAT_TOOL_ROUNDS.saturating_sub(round),
            );
            telemetry.finish_round_post(
                core,
                request_id,
                &conversation,
                round,
                post_round_started,
                false,
                true,
            );
            continue;
        }
        if let Some(terminal) = terminal_without_tool_calls(&mut generated, &tool_state.plan_policy)
        {
            telemetry.finish_round_post(
                core,
                request_id,
                &conversation,
                round,
                post_round_started,
                false,
                false,
            );
            return terminal;
        }
        trace_emitted_tool_calls(core, request_id, &conversation, round, &generated.calls);
        if generated.reason != GenerationCompletion::Stop
            || !tool_state.admits_tool_batch(round, &generated.calls)
        {
            telemetry.finish_round_post(
                core,
                request_id,
                &conversation,
                round,
                post_round_started,
                false,
                false,
            );
            return plan_pending::tool_limit_terminal(tool_state, round, &generated.calls);
        }
        match validate_or_recover_phase_calls(
            app,
            core,
            request_id,
            sequence,
            &mut conversation,
            &workspaces,
            &tools,
            &all_tools,
            tool_state,
            &mut generated,
            round,
            cancellation_requested,
        ) {
            Ok(true) => {
                telemetry.finish_round_post(
                    core,
                    request_id,
                    &conversation,
                    round,
                    post_round_started,
                    true,
                    true,
                );
                continue;
            }
            Ok(false) => {}
            Err(terminal) => {
                telemetry.finish_round_post(
                    core,
                    request_id,
                    &conversation,
                    round,
                    post_round_started,
                    true,
                    true,
                );
                return terminal;
            }
        }
        if generated
            .calls
            .iter()
            .any(|call| call.name == "orchestration.recall")
        {
            let terminal = dispatch::recall::execute(
                app,
                core,
                request_id,
                sequence,
                &mut conversation,
                generated,
            );
            telemetry.finish_round_post(
                core,
                request_id,
                &conversation,
                round,
                post_round_started,
                true,
                false,
            );
            if let Some(terminal) = terminal {
                return terminal;
            }
            continue;
        }
        if generated.calls.iter().any(is_stage_completion_call) {
            let terminal = execute_positioned_stage_completion_round(
                app,
                core,
                request_id,
                sequence,
                &mut conversation,
                &workspaces,
                tool_state,
                telemetry,
                generated,
                round,
                cancellation_requested,
            );
            telemetry.finish_round_post(
                core,
                request_id,
                &conversation,
                round,
                post_round_started,
                true,
                false,
            );
            if let Some(terminal) = terminal {
                return terminal;
            }
            continue;
        }
        if generated.calls.iter().any(is_tool_search_call) {
            let terminal = execute_tool_search_round(
                app,
                core,
                request_id,
                sequence,
                &mut conversation,
                &all_tools,
                tool_state,
                telemetry,
                generated,
                round,
            );
            telemetry.finish_round_post(
                core,
                request_id,
                &conversation,
                round,
                post_round_started,
                true,
                false,
            );
            if let Some(terminal) = terminal {
                return terminal;
            }
            continue;
        }
        if generated.calls.first().is_some_and(is_plan_call) {
            let corrections_before = tool_state.argument_corrections;
            let terminal = execute_plan_prefixed_round(
                app,
                core,
                request_id,
                sequence,
                &mut conversation,
                &workspaces,
                &all_tools,
                tool_state,
                telemetry,
                generated,
                round,
                cancellation_requested,
            );
            let recovered = tool_state.argument_corrections > corrections_before;
            telemetry.set_plan_revision(tool_state.plan_policy.revision());
            telemetry.finish_round_post(
                core,
                request_id,
                &conversation,
                round,
                post_round_started,
                true,
                recovered,
            );
            if let Some(terminal) = terminal {
                return terminal;
            }
            continue;
        }
        if generated.calls.first().is_some_and(is_forecast_call) {
            let corrections_before = tool_state.argument_corrections;
            let terminal = execute_forecast_prefixed_round(
                app,
                core,
                request_id,
                sequence,
                &mut conversation,
                &workspaces,
                &all_tools,
                tool_state,
                telemetry,
                generated,
                round,
                cancellation_requested,
            );
            let recovered = tool_state.argument_corrections > corrections_before;
            telemetry.finish_round_post(
                core,
                request_id,
                &conversation,
                round,
                post_round_started,
                true,
                recovered,
            );
            if let Some(terminal) = terminal {
                return terminal;
            }
            continue;
        }
        let terminal = execute_workspace_tool_round(
            app,
            core,
            request_id,
            sequence,
            &mut conversation,
            &workspaces,
            tool_state,
            telemetry,
            generated,
            round,
            cancellation_requested,
        );
        telemetry.finish_round_post(
            core,
            request_id,
            &conversation,
            round,
            post_round_started,
            true,
            false,
        );
        if let Some(terminal) = terminal {
            return terminal;
        }
    }
    plan_pending::tool_limit_terminal(tool_state, MAX_CHAT_TOOL_ROUNDS, &[])
}

#[expect(
    clippy::too_many_arguments,
    reason = "generation admission binds independent trusted request components at one boundary"
)]
pub(super) fn prepare_chat_generation(
    app: &tauri::AppHandle,
    core: &AppCore,
    executable: &Path,
    request_id: &RequestId,
    conversation: &Conversation,
    tools: Vec<PromptToolDefinition>,
    presentation: &RichInputPresentationPolicy,
    cancellation_requested: &AtomicBool,
    round: usize,
    orchestration: ToolOrchestrationContext,
    execution_profile: ExecutionProfileDecision,
    sequence: &mut u32,
    settings: ChatGenerationSettings,
    continuation: &app_core::ChatPromptContinuation,
) -> Result<(PreparedChatGeneration, u128), ChatTerminal> {
    if cancellation_requested.load(Ordering::Acquire) {
        return Err(ChatTerminal::Cancelled);
    }
    let model_admission_started = Instant::now();
    let output_tokens = core
        .conversation_model(&conversation.id)
        .and_then(|selection| selection.ok_or(AppCoreError::InstalledModelNotFound))
        .and_then(|selection| {
            core.ensure_selected_chat_model_loaded_for_assets(
                executable,
                &selection,
                conversation_image_count(conversation) > 0,
            )
        })
        .map_err(|error| {
            trace_prompt_preparation_failure(
                core,
                request_id,
                conversation,
                round,
                "model_admission",
                false,
                &error,
            );
            terminal_error(
                generation_error(error),
                GenerationFailureCategory::PromptPreparation,
            )
        })?;
    let model_admission_ms = model_admission_started.elapsed().as_millis();
    emit_chat_phase(
        app,
        &conversation.id,
        request_id,
        sequence,
        ChatGenerationPhase::PreparingConversationContext,
    );
    if cancellation_requested.load(Ordering::Acquire) {
        return Err(ChatTerminal::Cancelled);
    }
    let duplicate_tool_definition = has_duplicate_tool_names(&tools);
    let prepared = core
        .prepare_chat_prompt_with_continuation(
            &request_id.0,
            conversation,
            &OffsetDateTime::now_utc().date().to_string(),
            tools,
            output_tokens,
            orchestration,
            execution_profile,
            presentation,
            Some(cancellation_requested),
            settings.context_tokens,
            Some(continuation),
        )
        .map_err(|error| {
            if matches!(
                error,
                AppCoreError::Inference(InferenceError::Cancelled)
                    | AppCoreError::Ollama(model_runtime::OllamaError::Cancelled)
            ) {
                ChatTerminal::Cancelled
            } else {
                trace_prompt_preparation_failure(
                    core,
                    request_id,
                    conversation,
                    round,
                    "prompt_render",
                    duplicate_tool_definition,
                    &error,
                );
                terminal_error(
                    generation_error(error),
                    GenerationFailureCategory::PromptPreparation,
                )
            }
        })?;
    if cancellation_requested.load(Ordering::Acquire) {
        return Err(ChatTerminal::Cancelled);
    }
    Ok((prepared, model_admission_ms))
}

fn has_duplicate_tool_names(tools: &[PromptToolDefinition]) -> bool {
    let mut names = BTreeSet::new();
    tools.iter().any(|tool| !names.insert(tool.name.as_str()))
}

#[cfg(any(debug_assertions, test))]
fn prompt_preparation_failure_subtype(
    error: &AppCoreError,
    duplicate_tool_definition: bool,
) -> &'static str {
    if duplicate_tool_definition {
        return "duplicate_tool_definition";
    }
    match error {
        AppCoreError::ModelPrompt(error) => match error.kind() {
            PromptAdapterFailureKind::InvalidHistory => "invalid_history",
            PromptAdapterFailureKind::ToolLimit => "tool_limit",
            PromptAdapterFailureKind::InvalidContent => "invalid_content",
            PromptAdapterFailureKind::InputLimit => "input_limit",
            PromptAdapterFailureKind::OutputLimit => "output_limit",
            PromptAdapterFailureKind::InvalidDate => "invalid_date",
            PromptAdapterFailureKind::InvalidToolDefinition => "invalid_tool_definition",
            PromptAdapterFailureKind::InvalidToolCall => "invalid_tool_call",
            PromptAdapterFailureKind::Serialization => "serialization",
        },
        AppCoreError::ContextAdmission(_) => "context_admission",
        AppCoreError::WorkingContext(agent_core::WorkingContextError::ContextCapacityGap) => {
            "context_capacity_gap"
        }
        AppCoreError::WorkingContext(_) => "working_context",
        AppCoreError::Tokenizer(_) => "tokenizer",
        AppCoreError::InstalledModelNotFound => "installed_model_not_found",
        AppCoreError::InstalledModelInvalid => "installed_model_invalid",
        AppCoreError::VisionProjectorNotInstalled => "vision_projector_not_installed",
        AppCoreError::InvalidInferenceLimits => "invalid_inference_limits",
        AppCoreError::UnsupportedConversationContent => "unsupported_conversation_content",
        AppCoreError::AgentSystemInstructions(_) => "agent_system_instructions",
        AppCoreError::Inference(InferenceError::Cancelled) => "cancelled",
        AppCoreError::Inference(_) => "model_load",
        _ => "other",
    }
}

#[cfg(debug_assertions)]
#[allow(clippy::too_many_arguments)]
fn trace_prompt_preparation_failure(
    core: &AppCore,
    request_id: &RequestId,
    conversation: &Conversation,
    round: usize,
    stage: &'static str,
    duplicate_tool_definition: bool,
    error: &AppCoreError,
) {
    core.record_development_tool_trace(&serde_json::json!({
        "event": "execution_prompt_preparation_failed",
        "requestId": request_id.0,
        "conversationId": conversation.id.0,
        "round": round,
        "stage": stage,
        "subtype": prompt_preparation_failure_subtype(error, duplicate_tool_definition)
    }));
}

#[cfg(not(debug_assertions))]
#[allow(clippy::too_many_arguments)]
fn trace_prompt_preparation_failure(
    _core: &AppCore,
    _request_id: &RequestId,
    _conversation: &Conversation,
    _round: usize,
    _stage: &'static str,
    _duplicate_tool_definition: bool,
    _error: &AppCoreError,
) {
}

fn is_discovery_call(call: &ParsedToolCall) -> bool {
    matches!(
        call.name.as_str(),
        "filesystem.list"
            | "filesystem.find_paths"
            | "filesystem.collect_evidence"
            | "filesystem.stat"
            | "filesystem.read_text"
            | "filesystem.inspect_image"
            | "filesystem.search"
            | "filesystem.scan"
    )
}

#[cfg(test)]
mod tests;
