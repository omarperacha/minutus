//! One prepared model-stream round and its output accounting.

#[cfg(debug_assertions)]
use super::state::longest_common_rendered_prefix;
use super::stream_display::RoundStreamDisplay;
use super::*;

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
pub(super) fn execute_chat_generation_round(
    app: &tauri::AppHandle,
    core: &AppCore,
    executable: &Path,
    request_id: &RequestId,
    conversation: &Conversation,
    tools: Vec<PromptToolDefinition>,
    presentation: &RichInputPresentationPolicy,
    cancellation_requested: &AtomicBool,
    sequence: &mut u32,
    model_passes: &mut Vec<TaskActivity>,
    round: usize,
    orchestration: ToolOrchestrationContext,
    execution_profile: ExecutionProfileDecision,
    rich_input_reinspection: bool,
    provider_prefix_boundary: ProviderPromptPrefixBoundary,
    provider_prefix: &mut ProviderPromptPrefixTracker,
    tool_state: &mut GenerationToolState,
    settings: ChatGenerationSettings,
) -> Result<ChatGenerationRound, ChatTerminal> {
    let mut display = RoundStreamDisplay::new(request_id.clone(), round);
    emit_chat_event(app, &conversation.id, display.reset(sequence));
    emit_model_pass_activity(
        app,
        &conversation.id,
        request_id,
        sequence,
        model_pass_activity(
            round,
            TaskActivityStatus::InProgress,
            "Preparing next pass".to_owned(),
        ),
    );
    emit_chat_phase(
        app,
        &conversation.id,
        request_id,
        sequence,
        ChatGenerationPhase::LoadingLocalModel,
    );
    let preparation_started = Instant::now();
    trace_model_round_preparation_started(core, request_id, conversation, round);
    #[cfg(debug_assertions)]
    let active_tool_count = tools.len();
    #[cfg(debug_assertions)]
    let active_tool_contract_bytes = tools.iter().fold(0_usize, |total, tool| {
        total
            .saturating_add(tool.name.len())
            .saturating_add(tool.description.len())
            .saturating_add(tool.parameters.to_string().len())
    });
    let (prepared, model_admission_ms) = prepare_chat_generation(
        app,
        core,
        executable,
        request_id,
        conversation,
        tools,
        presentation,
        cancellation_requested,
        round,
        orchestration,
        execution_profile,
        sequence,
        settings,
        &tool_state.prompt_continuation,
    )?;
    let preparation_ms = preparation_started.elapsed().as_millis();
    #[cfg(debug_assertions)]
    let admitted_prompt_tokens = prepared.admitted_prompt_tokens;
    #[cfg(debug_assertions)]
    let provider_prompt_render_ms = prepared
        .prompt_preparation_timing
        .provider_prompt_render_ms();
    #[cfg(not(debug_assertions))]
    let provider_prompt_render_ms = 0;
    #[cfg(debug_assertions)]
    let exact_tokenization_ms = prepared.prompt_preparation_timing.exact_tokenization_ms();
    #[cfg(not(debug_assertions))]
    let exact_tokenization_ms = 0;
    #[cfg(debug_assertions)]
    let selected_execution_profile = prepared.execution_profile;
    #[cfg(debug_assertions)]
    let reasoning_strength = prepared.reasoning_strength;
    #[cfg(debug_assertions)]
    let profile_output_tokens = prepared.profile_output_tokens;
    #[cfg(debug_assertions)]
    let reasoning_reserve_tokens = prepared.reasoning_reserve_tokens;
    #[cfg(debug_assertions)]
    let final_answer_output_override = prepared.final_answer_output_override;
    #[cfg(debug_assertions)]
    let rich_evidence_checkpoint_reused = prepared.rich_evidence_checkpoint_reused;
    #[cfg(debug_assertions)]
    let first_user_boundary_tokens = prepared
        .rendered_provider_prompt
        .find("<|im_start|>user\n")
        .and_then(|boundary| {
            core.exact_generated_text_tokens(
                &prepared.binding,
                &prepared.rendered_provider_prompt[..boundary],
            )
            .ok()
        });
    #[cfg(debug_assertions)]
    let raw_shared_prefix_tokens = provider_prefix
        .previous
        .as_ref()
        .and_then(|(_, previous)| {
            longest_common_rendered_prefix(previous, &prepared.rendered_provider_prompt)
        })
        .and_then(|prefix| {
            core.exact_generated_text_tokens(&prepared.binding, &prefix)
                .ok()
        });
    #[cfg(debug_assertions)]
    let eligible_prefix_tokens = provider_prefix
        .observe(provider_prefix_boundary, &prepared.rendered_provider_prompt)
        .and_then(|prefix| {
            core.exact_generated_text_tokens(&prepared.binding, &prefix)
                .ok()
        });
    #[cfg(not(debug_assertions))]
    let _ = (
        rich_input_reinspection,
        provider_prefix_boundary,
        provider_prefix,
    );
    #[cfg(debug_assertions)]
    let (
        selected_history_messages,
        compacted_history_messages,
        dialogue_checkpoint_advanced,
        narrative_checkpoint,
    ) = (
        prepared.selected_history_messages,
        prepared.compacted_history_messages,
        prepared.dialogue_checkpoint_advanced,
        prepared.narrative_checkpoint,
    );
    let PreparedChatGeneration {
        prompt_continuation,
        prompt,
        asset_ids,
        binding,
        context_tokens,
        output_tokens,
        admitted_prompt_tokens: _,
        prompt_preparation_timing: _,
        rendered_provider_prompt: _,
        execution_profile: _,
        reasoning_strength: _,
        profile_output_tokens: _,
        reasoning_reserve_tokens: _,
        final_answer_output_override: _,
        rich_evidence_checkpoint_reused: _,
        selected_history_messages: _,
        compacted_history_messages: _,
        dialogue_checkpoint_advanced: _,
        narrative_checkpoint: _,
    } = prepared;
    #[cfg(debug_assertions)]
    let admission = RoundAdmissionTelemetry {
        model_id: binding.model_id().to_owned(),
        model_version: binding.model_version().to_owned(),
        adapter: binding.adapter().diagnostic_name(),
        provider: binding.provider().diagnostic_name(),
        context_tokens,
        output_tokens,
        admitted_prompt_tokens,
        eligible_prefix_tokens,
        raw_shared_prefix_tokens,
        first_user_boundary_tokens,
        active_tool_count,
        active_tool_contract_bytes,
        image_count: asset_ids.len(),
        execution_profile: selected_execution_profile.profile.diagnostic_name(),
        execution_work: selected_execution_profile.work.diagnostic_name(),
        policy_version: selected_execution_profile.policy_version,
        bounded_stage_action: selected_execution_profile.bounded_action,
        reasoning_strength: reasoning_strength.diagnostic_name(),
        profile_output_tokens,
        reasoning_reserve_tokens,
        final_answer_output_override,
        rich_evidence_checkpoint_reused,
        selected_history_messages,
        compacted_history_messages,
        dialogue_checkpoint_advances: usize::from(dialogue_checkpoint_advanced),
        narrative_checkpoint,
        rich_input_admission: RichInputAdmission::from_reinspection(rich_input_reinspection)
            .with_recovery(
                tool_state
                    .rich_input_presentations
                    .requests_recovery_for(&asset_ids),
            ),
    };
    #[cfg(not(debug_assertions))]
    let admission = RoundAdmissionTelemetry;
    let started = Instant::now();
    trace_model_round_started(core, request_id, conversation, round, &admission);
    let provider_detail = if preparation_ms >= 1_000 {
        format!(
            "Prepared in {} · Reading context and generating",
            format_model_pass_duration(u64::try_from(preparation_ms).unwrap_or(u64::MAX))
        )
    } else {
        "Reading context and generating".to_owned()
    };
    emit_model_pass_activity(
        app,
        &conversation.id,
        request_id,
        sequence,
        model_pass_activity(round, TaskActivityStatus::InProgress, provider_detail),
    );
    emit_chat_phase(
        app,
        &conversation.id,
        request_id,
        sequence,
        ChatGenerationPhase::ProcessingContext,
    );
    let mut stream_progress = ModelStreamProgress::new();
    let mut reasoning = String::new();
    let mut reasoning_filter = ReasoningEchoFilter::new(latest_user_text(conversation));
    let mut content = String::new();
    let mut calls = Vec::new();
    let mut completion = None;
    let mut stream_failure = None;
    let mut metrics = None;
    let mut provider_responded = false;
    trace_model_round_provider_requested(core, request_id, conversation, round);
    let result = core.start_generation_with_assets(
        &request_id.0,
        prompt,
        context_tokens,
        output_tokens,
        &binding,
        &asset_ids,
        |event| {
            if let Some(display_event) = display.project(&event, sequence) {
                emit_chat_event(app, &conversation.id, display_event);
            }
            match event {
                ModelGenerationEvent::OutputStarted => {
                    provider_responded = true;
                    stream_progress.output_started(core, request_id, conversation, round);
                    emit_model_pass_activity(
                        app,
                        &conversation.id,
                        request_id,
                        sequence,
                        model_pass_activity(
                            round,
                            TaskActivityStatus::InProgress,
                            "Receiving model output".to_owned(),
                        ),
                    );
                    emit_chat_phase(
                        app,
                        &conversation.id,
                        request_id,
                        sequence,
                        ChatGenerationPhase::Thinking,
                    );
                }
                ModelGenerationEvent::ContentPreviewDelta { .. } => {
                    provider_responded = true;
                    stream_progress.observe(core, request_id, conversation, round, "preview");
                }
                ModelGenerationEvent::ContentPreviewReset => {}
                ModelGenerationEvent::ReasoningDelta { text } => {
                    provider_responded = true;
                    stream_progress.observe(core, request_id, conversation, round, "analysis");
                    // Preserve the provider channel for replay; echo filtering
                    // belongs only to the UI projection.
                    reasoning.push_str(&text);
                    if let Some(text) = reasoning_filter.push(&text) {
                        emit_chat_event(
                            app,
                            &conversation.id,
                            GenerationEvent::ReasoningDelta {
                                request_id: request_id.clone(),
                                sequence: next_sequence(sequence),
                                text,
                            },
                        );
                    }
                }
                ModelGenerationEvent::FinalContentDelta { text } => {
                    provider_responded = true;
                    stream_progress.observe(core, request_id, conversation, round, "final");
                    content.push_str(&text);
                }
                ModelGenerationEvent::ToolCall { mut call } => {
                    provider_responded = true;
                    call.call_id = format!("{}:round:{}", call.call_id, round + 1);
                    calls.push(call);
                }
                ModelGenerationEvent::Metrics { metrics: value } => metrics = Some(value),
                ModelGenerationEvent::Completed { reason } => {
                    provider_responded = true;
                    completion = Some(reason);
                }
                ModelGenerationEvent::Cancelled => {
                    stream_failure = Some(ModelOutputFailureKind::Cancelled);
                }
                ModelGenerationEvent::Failed { failure } => stream_failure = Some(failure),
            }
        },
    );
    // A prepared prompt or a successful file read is not a presentation. Keep
    // pending requests on preparation/connection failure; a model response is
    // confirmation even if the stream is subsequently cancelled or rejected.
    if provider_responded {
        tool_state
            .record_image_presentation(core, &asset_ids, &binding.identity_sha256(), round)
            .map_err(|error| {
                terminal_error(
                    generation_error(error),
                    GenerationFailureCategory::PromptPreparation,
                )
            })?;
    }
    let stream_failure = resolved_stream_failure(&result, stream_failure);
    if provider_responded && (stream_failure.is_some() || result.is_err()) {
        tool_state
            .rich_input_presentations
            .request_recovery_images(&asset_ids);
    }
    if cancellation_requested.load(Ordering::Acquire)
        || matches!(
            &result,
            Err(AppCoreError::Inference(InferenceError::Cancelled))
        )
        || matches!(stream_failure, Some(ModelOutputFailureKind::Cancelled))
    {
        return Err(ChatTerminal::Cancelled);
    }
    if let Some(error) = stream_failure {
        trace_model_round_failed(
            core,
            request_id,
            conversation,
            round,
            &admission,
            &stream_progress,
            error,
            preparation_ms,
        );
        trace_model_stream_rejection(core, request_id, conversation, round, error);
        return Err(terminal_error(
            stream_error(error),
            GenerationFailureCategory::Inference,
        ));
    }
    result.map_err(|error| {
        terminal_error(
            generation_error(error),
            GenerationFailureCategory::Inference,
        )
    })?;
    let generation_call_ms = elapsed_ms(started);
    let reason = completion.ok_or_else(|| {
        terminal_error(
            IpcError {
                code: IpcErrorCode::Internal,
                message: "The response ended unexpectedly. You can try again.".to_owned(),
                retryable: true,
            },
            GenerationFailureCategory::Inference,
        )
    })?;
    if reason == GenerationCompletion::Stop
        && let Some(continuation) = prompt_continuation
    {
        tool_state.prompt_continuation = continuation;
    }
    #[allow(unused_mut)]
    let mut generated = ChatGenerationRound {
        reasoning,
        content,
        calls,
        reason,
        image_ids: asset_ids,
        binding_sha256: binding.identity_sha256(),
        #[cfg(debug_assertions)]
        timing: RoundTiming {
            preparation: u64::try_from(preparation_ms).unwrap_or(u64::MAX),
            generation_call: generation_call_ms,
            output_token_measurement: 0,
            provider_total: metrics.and_then(|value| value.total_ns).map(ns_to_ms),
        },
    };
    #[cfg(debug_assertions)]
    let output_token_measurement_started = Instant::now();
    #[cfg(debug_assertions)]
    let reasoning_tokens = core
        .exact_generated_text_tokens(&binding, &generated.reasoning)
        .ok();
    #[cfg(not(debug_assertions))]
    let reasoning_tokens = None;
    #[cfg(debug_assertions)]
    let final_output_tokens = core
        .exact_generated_text_tokens(&binding, &generated.content)
        .ok();
    #[cfg(not(debug_assertions))]
    let final_output_tokens = None;
    #[cfg(debug_assertions)]
    let output_token_measurement_ms = elapsed_ms(output_token_measurement_started);
    #[cfg(not(debug_assertions))]
    let output_token_measurement_ms = 0;
    #[cfg(debug_assertions)]
    {
        generated.timing.output_token_measurement = output_token_measurement_ms;
    }
    trace_model_round_completed(
        core,
        request_id,
        conversation,
        round,
        &generated,
        &admission,
        metrics,
        preparation_ms,
        model_admission_ms,
        provider_prompt_render_ms,
        exact_tokenization_ms,
        u128::from(generation_call_ms),
        output_token_measurement_ms,
        reasoning_tokens,
        final_output_tokens,
    );
    let detail = model_pass_timing_detail(metrics, preparation_ms);
    let activity = model_pass_activity(round, TaskActivityStatus::Completed, detail);
    emit_model_pass_activity(
        app,
        &conversation.id,
        request_id,
        sequence,
        activity.clone(),
    );
    model_passes.push(activity);
    Ok(generated)
}

pub(super) fn model_pass_activity(
    round: usize,
    status: TaskActivityStatus,
    detail: String,
) -> TaskActivity {
    TaskActivity {
        id: format!("model-pass-{round}"),
        kind: TaskActivityKind::ModelPass,
        title: format!("Model pass {}", round + 1),
        detail: Some(detail),
        status,
    }
}

pub(super) fn format_model_pass_duration(ms: u64) -> String {
    let seconds = ms.saturating_add(500) / 1_000;
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    }
}

fn model_pass_timing_detail(metrics: Option<GenerationMetrics>, preparation_ms: u128) -> String {
    let mut parts = Vec::new();
    if preparation_ms >= 1_000 {
        parts.push(format!(
            "Preparing {}",
            format_model_pass_duration(u64::try_from(preparation_ms).unwrap_or(u64::MAX))
        ));
    }
    if let Some(metrics) = metrics {
        if let Some(ns) = metrics.prompt_evaluation_ns {
            parts.push(format!(
                "Reading context {}",
                format_model_pass_duration(ns / 1_000_000)
            ));
        }
        if let Some(ns) = metrics.generation_ns {
            parts.push(format!(
                "Generating {}",
                format_model_pass_duration(ns / 1_000_000)
            ));
        }
    }
    if parts.is_empty() {
        parts.push("Provider timing unavailable".to_owned());
    }
    parts.join(" · ")
}

fn emit_model_pass_activity(
    app: &tauri::AppHandle,
    conversation_id: &ipc_types::ConversationId,
    request_id: &RequestId,
    sequence: &mut u32,
    activity: TaskActivity,
) {
    emit_chat_event(
        app,
        conversation_id,
        GenerationEvent::Activity {
            request_id: request_id.clone(),
            sequence: next_sequence(sequence),
            activity,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::{format_model_pass_duration, model_pass_timing_detail};
    use model_runtime::GenerationMetrics;

    #[test]
    fn completed_pass_distinguishes_evaluation_from_silent_generation() {
        let detail = model_pass_timing_detail(
            Some(GenerationMetrics {
                prompt_evaluation_ns: Some(261_640_000_000),
                generation_ns: Some(167_449_000_000),
                ..GenerationMetrics::default()
            }),
            315,
        );
        assert_eq!(detail, "Reading context 4m 22s · Generating 2m 47s");
        assert_eq!(format_model_pass_duration(325_201), "5m 25s");
    }
}
