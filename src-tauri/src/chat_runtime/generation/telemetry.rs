//! Generation-round measurements, traces, and terminal mutation reporting.

#[cfg(debug_assertions)]
use super::*;
#[cfg(not(debug_assertions))]
use super::{
    AppCore, AppCoreError, ChatTerminal, ChatToolExchange, Conversation, GenerationCompletion,
    GenerationFailureCategory, GenerationMetrics, Instant, IpcError, IpcErrorCode,
    MAX_CHAT_TOOL_CALLS, ModelOutputFailureKind, ParsedToolCall, PlanDefectProgress, PlanPolicy,
    PromptToolDefinition, RequestId, ToolActionClass, is_discovery_call, terminal_error,
    tool_action_class,
};

mod request;

#[cfg(debug_assertions)]
pub(super) use request::RoundTiming;
pub(super) use request::{RequestExecutionTelemetry, elapsed_ms};

pub(super) fn resolved_stream_failure(
    result: &Result<(), AppCoreError>,
    failure: Option<ModelOutputFailureKind>,
) -> Option<ModelOutputFailureKind> {
    if failure != Some(ModelOutputFailureKind::UpstreamFailure) {
        return failure;
    }
    match result {
        Err(AppCoreError::Ollama(error)) => Some((*error).into()),
        Err(AppCoreError::OllamaServiceUnavailable | AppCoreError::OllamaSidecar(_)) => {
            Some(ModelOutputFailureKind::ProviderUnavailable)
        }
        _ => failure,
    }
}

#[cfg(debug_assertions)]
#[allow(clippy::too_many_arguments)]
pub(super) fn model_round_completed_trace_data(
    request_id: &RequestId,
    conversation: &Conversation,
    round: usize,
    generated: &ChatGenerationRound,
    admission: &RoundAdmissionTelemetry,
    metrics: Option<GenerationMetrics>,
    preparation_ms: u128,
    model_admission_ms: u128,
    provider_prompt_render_ms: u64,
    exact_tokenization_ms: u64,
    elapsed_ms: u128,
    output_token_measurement_ms: u64,
    reasoning_tokens: Option<u64>,
    final_output_tokens: Option<u64>,
) -> serde_json::Value {
    let prompt_evaluation_ms = metrics
        .and_then(|value| value.prompt_evaluation_ns)
        .map(ns_to_ms);
    let model_generation_ms = metrics.and_then(|value| value.generation_ns).map(ns_to_ms);
    let provider_total_ms = metrics.and_then(|value| value.total_ns).map(ns_to_ms);
    let provider_overhead_ms = metrics.and_then(provider_overhead_ms);
    let generation_tokens_per_second = metrics.and_then(|value| {
        value
            .generated_tokens?
            .checked_mul(1_000_000_000)?
            .checked_div(value.generation_ns?)
    });
    let mut value = serde_json::json!({
        "event": "execution_round_completed",
        "requestId": request_id.0,
        "conversationId": conversation.id.0,
        "round": round,
        "modelId": admission.model_id,
        "modelVersion": admission.model_version,
        "adapter": admission.adapter,
        "provider": admission.provider,
        "contextTokenLimit": admission.context_tokens,
        "outputTokenLimit": admission.output_tokens,
        "admittedPromptTokens": admission.admitted_prompt_tokens,
        "eligiblePrefixTokens": admission.eligible_prefix_tokens,
        "providerPromptTokens": metrics.and_then(|value| value.prompt_tokens),
        "providerCachedPromptTokens": metrics.and_then(|value| value.cached_prompt_tokens),
        "providerCacheWritePromptTokens": metrics.and_then(|value| value.cache_write_prompt_tokens),
        "providerGeneratedTokens": metrics.and_then(|value| value.generated_tokens),
        "imageCount": admission.image_count,
        "executionProfile": admission.execution_profile,
        "executionWork": admission.execution_work,
        "executionProfilePolicyVersion": admission.policy_version,
        "boundedStageAction": admission.bounded_stage_action,
        "reasoningStrength": admission.reasoning_strength,
        "profileOutputTokenLimit": admission.profile_output_tokens,
        "reasoningReserveTokens": admission.reasoning_reserve_tokens,
        "finalAnswerOutputOverride": admission.final_answer_output_override,
        "richEvidenceCheckpointReused": admission.rich_evidence_checkpoint_reused,
        "richInputReinspection": admission.rich_input_admission.is_reinspection(),
        "reasoningTokens": reasoning_tokens,
        "finalOutputTokens": final_output_tokens,
        "completionReason": completion_reason_name(generated.reason),
        "toolCallCount": generated.calls.len(),
        "promptPreparationMs": preparation_ms,
        "promptEvaluationMs": prompt_evaluation_ms,
        "modelGenerationMs": model_generation_ms,
        "providerOverheadMs": provider_overhead_ms,
        "providerTotalMs": provider_total_ms,
        "providerLoadMs": metrics.and_then(|value| value.load_ns).map(ns_to_ms),
        "hostGenerationCallMs": elapsed_ms,
        "outputTokenMeasurementMs": output_token_measurement_ms,
        "generationTokensPerSecond": generation_tokens_per_second,
        "peakMemoryBytes": metrics.and_then(|value| value.peak_memory_bytes)
    });
    value["selectedHistoryMessages"] = admission.selected_history_messages.into();
    value["rawSharedPrefixTokens"] = serde_json::json!(admission.raw_shared_prefix_tokens);
    value["firstUserBoundaryTokens"] = serde_json::json!(admission.first_user_boundary_tokens);
    value["compactedHistoryMessages"] = admission.compacted_history_messages.into();
    value["dialogueCheckpointAdvances"] = admission.dialogue_checkpoint_advances.into();
    value["narrativeCheckpoint"] = admission.narrative_checkpoint.into();
    value["modelAdmissionMs"] = serde_json::json!(model_admission_ms);
    value["providerPromptRenderMs"] = provider_prompt_render_ms.into();
    value["exactTokenizationMs"] = exact_tokenization_ms.into();
    value["richInputRecovery"] = admission.rich_input_admission.is_recovery().into();
    value["generatedReasoningBytes"] = generated.reasoning.len().into();
    value["generatedTextBytes"] = generated.content.len().into();
    value
}

#[cfg(debug_assertions)]
pub(super) const fn ns_to_ms(value: u64) -> u64 {
    value / 1_000_000
}

#[cfg(debug_assertions)]
pub(super) fn provider_overhead_ms(metrics: GenerationMetrics) -> Option<u64> {
    let measured = metrics
        .prompt_evaluation_ns?
        .checked_add(metrics.generation_ns?)?;
    metrics.total_ns?.checked_sub(measured).map(ns_to_ms)
}

#[cfg(debug_assertions)]
pub(super) struct RoundAdmissionTelemetry {
    pub(super) model_id: String,
    pub(super) model_version: String,
    pub(super) adapter: &'static str,
    pub(super) provider: &'static str,
    pub(super) context_tokens: u32,
    pub(super) output_tokens: u32,
    pub(super) admitted_prompt_tokens: u64,
    pub(super) eligible_prefix_tokens: Option<u64>,
    pub(super) raw_shared_prefix_tokens: Option<u64>,
    pub(super) first_user_boundary_tokens: Option<u64>,
    pub(super) active_tool_count: usize,
    pub(super) active_tool_contract_bytes: usize,
    pub(super) image_count: usize,
    pub(super) execution_profile: &'static str,
    pub(super) execution_work: &'static str,
    pub(super) policy_version: u16,
    pub(super) bounded_stage_action: bool,
    pub(super) reasoning_strength: &'static str,
    pub(super) profile_output_tokens: u32,
    pub(super) reasoning_reserve_tokens: u64,
    pub(super) final_answer_output_override: bool,
    pub(super) rich_evidence_checkpoint_reused: bool,
    pub(super) selected_history_messages: usize,
    pub(super) compacted_history_messages: usize,
    pub(super) dialogue_checkpoint_advances: usize,
    pub(super) narrative_checkpoint: &'static str,
    pub(super) rich_input_admission: RichInputAdmission,
}

#[cfg(debug_assertions)]
#[derive(Clone, Copy)]
pub(super) enum RichInputAdmission {
    Ordinary,
    Reinspection,
    Recovery,
}

#[cfg(debug_assertions)]
impl RichInputAdmission {
    pub(super) const fn with_recovery(self, recovery: bool) -> Self {
        if matches!(self, Self::Ordinary) && recovery {
            Self::Recovery
        } else {
            self
        }
    }

    pub(super) const fn is_recovery(self) -> bool {
        matches!(self, Self::Recovery)
    }

    pub(super) const fn from_reinspection(reinspection: bool) -> Self {
        if reinspection {
            Self::Reinspection
        } else {
            Self::Ordinary
        }
    }

    pub(super) const fn is_reinspection(self) -> bool {
        matches!(self, Self::Reinspection)
    }
}

#[cfg(not(debug_assertions))]
pub(super) struct RoundAdmissionTelemetry;

pub(super) struct ModelStreamProgress {
    #[cfg(debug_assertions)]
    channels: BTreeSet<&'static str>,
    #[cfg(debug_assertions)]
    pub(super) started: Instant,
    #[cfg(debug_assertions)]
    pub(super) last_trace: Instant,
    #[cfg(debug_assertions)]
    pub(super) delta_count: u64,
    #[cfg(debug_assertions)]
    first_output_ms: Option<u64>,
    #[cfg(debug_assertions)]
    last_delta_ms: Option<u64>,
}

impl ModelStreamProgress {
    pub(super) fn new() -> Self {
        Self {
            #[cfg(debug_assertions)]
            channels: BTreeSet::new(),
            #[cfg(debug_assertions)]
            started: Instant::now(),
            #[cfg(debug_assertions)]
            last_trace: Instant::now(),
            #[cfg(debug_assertions)]
            delta_count: 0,
            #[cfg(debug_assertions)]
            first_output_ms: None,
            #[cfg(debug_assertions)]
            last_delta_ms: None,
        }
    }

    pub(super) fn output_started(
        &mut self,
        core: &AppCore,
        request_id: &RequestId,
        conversation: &Conversation,
        round: usize,
    ) {
        #[cfg(debug_assertions)]
        {
            let first_output_ms = elapsed_ms(self.started);
            self.first_output_ms.get_or_insert(first_output_ms);
            core.record_development_tool_trace(&serde_json::json!({
                "event": "execution_round_provider_output_started",
                "requestId": request_id.0,
                "conversationId": conversation.id.0,
                "round": round,
                "elapsedMs": first_output_ms
            }));
        }
        #[cfg(not(debug_assertions))]
        let _ = (self, core, request_id, conversation, round);
    }

    pub(super) fn observe(
        &mut self,
        core: &AppCore,
        request_id: &RequestId,
        conversation: &Conversation,
        round: usize,
        channel: &'static str,
    ) {
        #[cfg(debug_assertions)]
        {
            self.delta_count = self.delta_count.saturating_add(1);
            let now = Instant::now();
            self.last_delta_ms = Some(elapsed_ms(self.started));
            let first_delta = self.delta_count == 1;
            let first_channel = self.channels.insert(channel);
            if first_delta
                || first_channel
                || now.duration_since(self.last_trace) >= STREAM_PROGRESS_INTERVAL
            {
                core.record_development_tool_trace(&serde_json::json!({
                    "event": "execution_round_stream_progress",
                    "requestId": request_id.0,
                    "conversationId": conversation.id.0,
                    "round": round,
                    "phase": if first_delta { "first_delta" } else if first_channel { "first_channel_delta" } else { "active" },
                    "channel": channel,
                    "elapsedMs": elapsed_ms(self.started),
                    "deltaCount": self.delta_count
                }));
                self.last_trace = now;
            }
        }
        #[cfg(not(debug_assertions))]
        let _ = (self, core, request_id, conversation, round, channel);
    }
}

#[cfg(debug_assertions)]
#[allow(clippy::too_many_arguments)]
pub(super) fn trace_model_round_failed(
    core: &AppCore,
    request_id: &RequestId,
    conversation: &Conversation,
    round: usize,
    admission: &RoundAdmissionTelemetry,
    progress: &ModelStreamProgress,
    failure: ModelOutputFailureKind,
    preparation_ms: u128,
) {
    core.record_development_tool_trace(&serde_json::json!({
        "event": "execution_round_failed",
        "requestId": request_id.0,
        "conversationId": conversation.id.0,
        "round": round,
        "failureSubtype": failure.code(),
        "promptPreparationMs": preparation_ms,
        "providerCallMs": elapsed_ms(progress.started),
        "firstProviderOutputMs": progress.first_output_ms,
        "lastProviderDeltaMs": progress.last_delta_ms,
        "providerDeltaCount": progress.delta_count,
        "admittedPromptTokens": admission.admitted_prompt_tokens,
        "eligiblePrefixTokens": admission.eligible_prefix_tokens,
        "rawSharedPrefixTokens": admission.raw_shared_prefix_tokens,
        "firstUserBoundaryTokens": admission.first_user_boundary_tokens,
        "outputTokenLimit": admission.output_tokens,
        "provider": admission.provider,
        "adapter": admission.adapter
    }));
}

#[cfg(not(debug_assertions))]
#[allow(clippy::too_many_arguments)]
pub(super) fn trace_model_round_failed(
    _core: &AppCore,
    _request_id: &RequestId,
    _conversation: &Conversation,
    _round: usize,
    _admission: &RoundAdmissionTelemetry,
    _progress: &ModelStreamProgress,
    _failure: ModelOutputFailureKind,
    _preparation_ms: u128,
) {
}

#[cfg(debug_assertions)]
pub(super) fn trace_model_round_preparation_started(
    core: &AppCore,
    request_id: &RequestId,
    conversation: &Conversation,
    round: usize,
) {
    core.record_development_tool_trace(&serde_json::json!({
        "event": "execution_round_preparation_started",
        "requestId": request_id.0,
        "conversationId": conversation.id.0,
        "round": round
    }));
}

#[cfg(not(debug_assertions))]
pub(super) fn trace_model_round_preparation_started(
    _core: &AppCore,
    _request_id: &RequestId,
    _conversation: &Conversation,
    _round: usize,
) {
}

#[cfg(debug_assertions)]
pub(super) fn trace_model_round_provider_requested(
    core: &AppCore,
    request_id: &RequestId,
    conversation: &Conversation,
    round: usize,
) {
    core.record_development_tool_trace(&serde_json::json!({
        "event": "execution_round_provider_requested",
        "requestId": request_id.0,
        "conversationId": conversation.id.0,
        "round": round
    }));
}

#[cfg(not(debug_assertions))]
pub(super) fn trace_model_round_provider_requested(
    _core: &AppCore,
    _request_id: &RequestId,
    _conversation: &Conversation,
    _round: usize,
) {
}

#[cfg(debug_assertions)]
pub(super) fn trace_model_round_started(
    core: &AppCore,
    request_id: &RequestId,
    conversation: &Conversation,
    round: usize,
    admission: &RoundAdmissionTelemetry,
) {
    core.record_development_tool_trace(&serde_json::json!({
        "event": "execution_round_started",
        "requestId": request_id.0,
        "conversationId": conversation.id.0,
        "round": round,
        "admittedPromptTokens": admission.admitted_prompt_tokens,
        "activeToolCount": admission.active_tool_count,
        "activeToolContractBytes": admission.active_tool_contract_bytes,
        "imageCount": admission.image_count,
        "executionProfile": admission.execution_profile,
        "executionWork": admission.execution_work,
        "reasoningStrength": admission.reasoning_strength
    }));
}

#[cfg(debug_assertions)]
pub(super) fn trace_response_persistence(
    core: &AppCore,
    request_id: &RequestId,
    duration_ms: u64,
    persisted: bool,
) {
    core.record_development_tool_trace(&serde_json::json!({
        "event": "execution_response_persisted",
        "requestId": request_id.0,
        "durationMs": duration_ms,
        "persisted": persisted
    }));
}

#[cfg(not(debug_assertions))]
pub(super) fn trace_response_persistence(
    _core: &AppCore,
    _request_id: &RequestId,
    _duration_ms: u64,
    _persisted: bool,
) {
}

#[cfg(not(debug_assertions))]
pub(super) fn trace_model_round_started(
    _core: &AppCore,
    _request_id: &RequestId,
    _conversation: &Conversation,
    _round: usize,
    _admission: &RoundAdmissionTelemetry,
) {
}

#[cfg(debug_assertions)]
#[allow(clippy::too_many_arguments)]
pub(super) fn trace_model_round_completed(
    core: &AppCore,
    request_id: &RequestId,
    conversation: &Conversation,
    round: usize,
    generated: &ChatGenerationRound,
    admission: &RoundAdmissionTelemetry,
    metrics: Option<GenerationMetrics>,
    preparation_ms: u128,
    model_admission_ms: u128,
    provider_prompt_render_ms: u64,
    exact_tokenization_ms: u64,
    elapsed_ms: u128,
    output_token_measurement_ms: u64,
    reasoning_tokens: Option<u64>,
    final_output_tokens: Option<u64>,
) {
    core.record_development_tool_trace(&model_round_completed_trace_data(
        request_id,
        conversation,
        round,
        generated,
        admission,
        metrics,
        preparation_ms,
        model_admission_ms,
        provider_prompt_render_ms,
        exact_tokenization_ms,
        elapsed_ms,
        output_token_measurement_ms,
        reasoning_tokens,
        final_output_tokens,
    ));
}

#[cfg(not(debug_assertions))]
#[allow(clippy::too_many_arguments)]
pub(super) fn trace_model_round_completed(
    _core: &AppCore,
    _request_id: &RequestId,
    _conversation: &Conversation,
    _round: usize,
    _generated: &ChatGenerationRound,
    _admission: &RoundAdmissionTelemetry,
    _metrics: Option<GenerationMetrics>,
    _preparation_ms: u128,
    _model_admission_ms: u128,
    _provider_prompt_render_ms: u64,
    _exact_tokenization_ms: u64,
    _elapsed_ms: u128,
    _output_token_measurement_ms: u64,
    _reasoning_tokens: Option<u64>,
    _final_output_tokens: Option<u64>,
) {
}

#[cfg(debug_assertions)]
#[allow(clippy::too_many_arguments)]
pub(super) fn trace_plan_outcome_recovery(
    core: &AppCore,
    request_id: &RequestId,
    conversation: &Conversation,
    round: usize,
    reason: GenerationCompletion,
    remaining_calls: usize,
    remaining_rounds: usize,
) {
    core.record_development_tool_trace(&serde_json::json!({
        "event": "plan_outcome_recovery_scheduled",
        "requestId": request_id.0,
        "conversationId": conversation.id.0,
        "round": round,
        "completionReason": completion_reason_name(reason),
        "remainingCalls": remaining_calls,
        "remainingRounds": remaining_rounds
    }));
}

#[cfg(not(debug_assertions))]
#[allow(clippy::too_many_arguments)]
pub(super) fn trace_plan_outcome_recovery(
    _core: &AppCore,
    _request_id: &RequestId,
    _conversation: &Conversation,
    _round: usize,
    _reason: GenerationCompletion,
    _remaining_calls: usize,
    _remaining_rounds: usize,
) {
}

#[cfg(debug_assertions)]
pub(super) fn trace_plan_repair_decision(
    core: &AppCore,
    request_id: &RequestId,
    conversation: &Conversation,
    round: usize,
    progress: Option<PlanDefectProgress>,
    resumed: bool,
) {
    core.record_development_tool_trace(&serde_json::json!({
        "event": "plan_repair_decision",
        "requestId": request_id.0,
        "conversationId": conversation.id.0,
        "round": round,
        "removedDefectCount": progress.map_or(0, |value| value.removed),
        "changedDefectCount": progress.map_or(0, |value| value.changed),
        "newlyVisibleDefectCount": progress.map_or(0, |value| value.newly_visible),
        "decision": if resumed { "resume" } else { "terminal" }
    }));
}

#[cfg(not(debug_assertions))]
pub(super) fn trace_plan_repair_decision(
    _core: &AppCore,
    _request_id: &RequestId,
    _conversation: &Conversation,
    _round: usize,
    _progress: Option<PlanDefectProgress>,
    _resumed: bool,
) {
}

#[cfg(debug_assertions)]
pub(super) fn trace_emitted_tool_calls(
    core: &AppCore,
    request_id: &RequestId,
    conversation: &Conversation,
    round: usize,
    calls: &[ParsedToolCall],
) {
    for call in calls {
        core.record_development_tool_trace(&serde_json::json!({
            "event": "model_tool_call_emitted",
            "requestId": request_id.0,
            "conversationId": conversation.id.0,
            "round": round,
            "callId": call.call_id,
            "recipient": call.recipient,
            "tool": call.name,
            "arguments": diagnostic_tool_arguments(call),
        }));
    }
}

#[cfg(not(debug_assertions))]
pub(super) fn trace_emitted_tool_calls(
    _core: &AppCore,
    _request_id: &RequestId,
    _conversation: &Conversation,
    _round: usize,
    _calls: &[ParsedToolCall],
) {
}

#[cfg(debug_assertions)]
#[allow(clippy::too_many_arguments)]
pub(super) fn trace_tool_rejection(
    core: &AppCore,
    request_id: &RequestId,
    conversation: &Conversation,
    round: usize,
    calls: &[ParsedToolCall],
    category: &str,
    message: &str,
) {
    core.record_development_tool_trace(&serde_json::json!({
        "event": "tool_call_rejected",
        "requestId": request_id.0,
        "conversationId": conversation.id.0,
        "round": round,
        "category": category,
        "message": message,
        "calls": calls.iter().map(|call| serde_json::json!({
            "callId": call.call_id,
            "recipient": call.recipient,
            "tool": call.name,
            "arguments": diagnostic_tool_arguments(call),
        })).collect::<Vec<_>>(),
    }));
}

#[cfg(not(debug_assertions))]
#[allow(clippy::too_many_arguments)]
pub(super) fn trace_tool_rejection(
    _core: &AppCore,
    _request_id: &RequestId,
    _conversation: &Conversation,
    _round: usize,
    _calls: &[ParsedToolCall],
    _category: &str,
    _message: &str,
) {
}

#[cfg(debug_assertions)]
pub(super) fn trace_tool_exchanges(
    core: &AppCore,
    request_id: &RequestId,
    conversation: &Conversation,
    round: usize,
    exchanges: &[Result<ChatToolExchange, IpcError>],
) {
    for exchange in exchanges {
        match exchange {
            Ok(exchange)
                if exchange.request.name == ipc_types::ToolName::OrchestrationSubmitForecast =>
            {
                core.record_development_tool_trace(&serde_json::json!({
                    "event": "tool_call_finished",
                    "requestId": request_id.0,
                    "conversationId": conversation.id.0,
                    "round": round,
                    "callId": exchange.request.call_id,
                    "tool": "orchestration.submit_forecast",
                    "outcome": if matches!(exchange.result.outcome, ToolOutcome::Completed { .. }) { "completed" } else { "failed" }
                }));
            }
            Ok(exchange) if exchange.request.name == ipc_types::ToolName::WalletRequestTransfer => {
                core.record_development_tool_trace(&serde_json::json!({
                    "event":"tool_call_finished", "requestId":request_id.0,
                    "conversationId":conversation.id.0, "round":round,
                    "callId":exchange.request.call_id, "tool":"wallet.request_transfer",
                    "outcome":if matches!(exchange.result.outcome, ToolOutcome::Completed {..}) {"completed"} else {"failed"}
                }));
            }
            Ok(exchange) if exchange.request.name == ipc_types::ToolName::X402Request => {
                core.record_development_tool_trace(&serde_json::json!({
                    "event": "tool_call_finished",
                    "requestId": request_id.0,
                    "conversationId": conversation.id.0,
                    "round": round,
                    "callId": exchange.request.call_id,
                    "tool": "x402.request",
                    "operation": exchange.request.arguments.get("operation"),
                    "continuationProvided": exchange.request.arguments.contains_key("resourceRef"),
                    "sourceResolvedFromContinuation": exchange.request.arguments.contains_key("resourceRef"),
                    "outcome": if matches!(exchange.result.outcome, ToolOutcome::Completed { .. }) { "completed" } else { "failed" }
                }));
            }
            Ok(exchange) => core.record_development_tool_trace(&serde_json::json!({
                "event": "tool_call_finished",
                "requestId": request_id.0,
                "conversationId": conversation.id.0,
                "round": round,
                "request": exchange.request,
                "result": exchange.result,
            })),
            Err(error) => core.record_development_tool_trace(&serde_json::json!({
                "event": "tool_call_internal_failure",
                "requestId": request_id.0,
                "conversationId": conversation.id.0,
                "round": round,
                "error": error,
            })),
        }
    }
}

#[cfg(debug_assertions)]
fn diagnostic_tool_arguments(call: &ParsedToolCall) -> serde_json::Value {
    if call.name == "wallet.request_transfer" {
        return serde_json::json!({"fieldNames":call.arguments.keys().collect::<Vec<_>>()});
    }
    if call.name != "x402.request" {
        return serde_json::json!(call.arguments);
    }
    serde_json::json!({
        "operation": call.arguments.get("operation"),
        "fieldNames": call.arguments.keys().collect::<Vec<_>>(),
        "continuationProvided": call.arguments.contains_key("resourceRef"),
        "inputProvided": call.arguments.contains_key("input"),
        "urlProvided": call.arguments.contains_key("url"),
    })
}

#[cfg(not(debug_assertions))]
pub(super) fn trace_tool_exchanges(
    _core: &AppCore,
    _request_id: &RequestId,
    _conversation: &Conversation,
    _round: usize,
    _exchanges: &[Result<ChatToolExchange, IpcError>],
) {
}

#[cfg(debug_assertions)]
pub(super) fn trace_execution_forecast(
    core: &AppCore,
    request_id: &RequestId,
    conversation: &Conversation,
    round: usize,
    plan: &PlanPolicy,
    outcome: &str,
) {
    core.record_development_tool_trace(&serde_json::json!({
        "event": "execution_forecast_admission",
        "requestId": request_id.0,
        "conversationId": conversation.id.0,
        "round": round,
        "outcome": outcome,
        "forecast": plan.forecast_telemetry(),
    }));
}

#[cfg(not(debug_assertions))]
pub(super) fn trace_execution_forecast(
    _core: &AppCore,
    _request_id: &RequestId,
    _conversation: &Conversation,
    _round: usize,
    _plan: &PlanPolicy,
    _outcome: &str,
) {
}

#[cfg(debug_assertions)]
#[allow(clippy::too_many_arguments)]
pub(super) fn trace_plan_tool_results(
    core: &AppCore,
    request_id: &RequestId,
    conversation: &Conversation,
    round: usize,
    plan: &PlanPolicy,
    calls: &[ParsedToolCall],
    exchanges: &[Result<ChatToolExchange, IpcError>],
) {
    let admissions = plan.admission_records(calls, exchanges);
    for ((call, exchange), admission) in calls.iter().zip(exchanges).zip(admissions) {
        let (revision, forecast_step, forecast_match) = plan.diagnostic_match(call);
        let (execution_forecast_match, execution_forecast_category) =
            plan.forecast_diagnostic(call);
        let status = match exchange {
            Ok(exchange) if exchange.activity.status == TaskActivityStatus::Reused => "reused",
            Ok(exchange) if matches!(exchange.result.outcome, ToolOutcome::Completed { .. }) => {
                match forecast_match {
                    ForecastDiagnosticMatch::Matched => "matched",
                    ForecastDiagnosticMatch::Unforecasted => "unforecasted",
                    ForecastDiagnosticMatch::OutOfOrder => "out_of_order",
                }
            }
            Ok(_) | Err(_) => "failed",
        };
        let progress_contribution = if call.name == COMPLETE_STAGE_TOOL
            && exchange.as_ref().is_ok_and(|exchange| {
                matches!(exchange.result.outcome, ToolOutcome::Completed { .. })
            }) {
            "stage_completed"
        } else if plan.uses_advisory_stages()
            && exchange.as_ref().is_ok_and(|exchange| {
                matches!(exchange.result.outcome, ToolOutcome::Completed { .. })
            })
            && admission.relationship.can_advance_current()
        {
            "result_attached"
        } else {
            admission.progress.as_str()
        };
        core.record_development_tool_trace(&serde_json::json!({
            "event": "execution_tool_call_classified",
            "requestId": request_id.0,
            "conversationId": conversation.id.0,
            "round": round,
            "tool": call.name,
            "planRevision": revision,
            "forecastStep": forecast_step,
            "status": status,
            "executionForecastRevision": plan.forecast_revision(),
            "executionForecastMatch": execution_forecast_match,
            "executionForecastCategory": execution_forecast_category,
            "outcomeCoverage": admission.progress.as_str(),
            "permissionOutcome": admission.permission.as_str(),
            "stepRelationship": admission.relationship.as_str(),
            "progressContribution": progress_contribution
        }));
    }
}

#[cfg(not(debug_assertions))]
#[allow(clippy::too_many_arguments)]
pub(super) fn trace_plan_tool_results(
    _core: &AppCore,
    _request_id: &RequestId,
    _conversation: &Conversation,
    _round: usize,
    _plan: &PlanPolicy,
    _calls: &[ParsedToolCall],
    _exchanges: &[Result<ChatToolExchange, IpcError>],
) {
}

#[cfg(debug_assertions)]
pub(super) fn trace_model_stream_rejection(
    core: &AppCore,
    request_id: &RequestId,
    conversation: &Conversation,
    round: usize,
    failure: ModelOutputFailureKind,
) {
    core.record_development_tool_trace(&model_stream_rejection_trace_data(
        request_id,
        conversation,
        round,
        failure,
    ));
}

#[cfg(not(debug_assertions))]
pub(super) fn trace_model_stream_rejection(
    _core: &AppCore,
    _request_id: &RequestId,
    _conversation: &Conversation,
    _round: usize,
    _failure: ModelOutputFailureKind,
) {
}

#[cfg(debug_assertions)]
pub(super) fn model_stream_rejection_trace_data(
    request_id: &RequestId,
    conversation: &Conversation,
    round: usize,
    failure: ModelOutputFailureKind,
) -> serde_json::Value {
    serde_json::json!({
        "event": "model_stream_rejected",
        "requestId": request_id.0,
        "conversationId": conversation.id.0,
        "round": round,
        "subtype": failure.code(),
    })
}

pub(super) fn remaining_plan_forecast_calls(
    total_calls: usize,
    current_batch_calls: usize,
) -> usize {
    let current_operation_calls = current_batch_calls.saturating_sub(1);
    let calls_before_current_operations = total_calls.saturating_sub(current_operation_calls);
    MAX_CHAT_TOOL_CALLS.saturating_sub(calls_before_current_operations)
}

pub(super) fn tool_capabilities(tools: &[PromptToolDefinition]) -> (bool, usize) {
    let workspace_mutation_tools_available = tools
        .iter()
        .any(|tool| tool_action_class(&tool.name) == Some(ToolActionClass::WorkspaceWrite));
    let outcome_tool_available = workspace_mutation_tools_available
        || tools
            .iter()
            .any(|tool| tool_action_class(&tool.name) == Some(ToolActionClass::ExternalWrite));
    (
        workspace_mutation_tools_available,
        usize::from(outcome_tool_available).saturating_add(1),
    )
}

pub(super) fn calls_available_in_phase(
    tools: &[PromptToolDefinition],
    calls: &[ParsedToolCall],
) -> bool {
    calls
        .iter()
        .all(|call| tools.iter().any(|tool| tool.name == call.name))
}

pub(super) fn next_consecutive_discovery_calls(current: usize, calls: &[ParsedToolCall]) -> usize {
    if calls.iter().all(is_discovery_call) {
        current.saturating_add(calls.len())
    } else {
        0
    }
}

pub(super) fn applied_change_confirmation(paths: &[String]) -> String {
    match paths {
        [path] => format!("Done — updated {path}."),
        paths => format!(
            "Done — updated {} workspace paths: {}.",
            paths.len(),
            paths.join(", ")
        ),
    }
}

pub(super) fn terminal_after_mutation(
    terminal: ChatTerminal,
    mutation_applied: bool,
) -> ChatTerminal {
    match (terminal, mutation_applied) {
        (ChatTerminal::Error { .. } | ChatTerminal::Cancelled, true) => terminal_error(
            applied_change_incomplete_error(),
            GenerationFailureCategory::Persistence,
        ),
        (terminal, _) => terminal,
    }
}

pub(super) fn applied_change_incomplete_error() -> IpcError {
    IpcError {
        code: IpcErrorCode::Unsupported,
        message: "The workspace change was applied, but Minutus could not finish its response. Check the affected file before retrying.".to_owned(),
        retryable: true,
    }
}

pub(super) struct ChatGenerationRound {
    pub(super) reasoning: String,
    pub(super) content: String,
    pub(super) calls: Vec<ParsedToolCall>,
    pub(super) reason: GenerationCompletion,
    pub(super) image_ids: Vec<ipc_types::AssetId>,
    pub(super) binding_sha256: String,
    #[cfg(debug_assertions)]
    pub(super) timing: RoundTiming,
}

#[cfg(all(test, debug_assertions))]
#[path = "wallet_transfer_trace_tests.rs"]
mod wallet_transfer_trace_tests;
