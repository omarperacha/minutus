//! Bounded repair and recovery for rejected model tool calls.

#[cfg(debug_assertions)]
use super::*;
#[cfg(not(debug_assertions))]
use super::{
    AppCore, AtomicBool, ChatGenerationRound, ChatTerminal, ChatToolExchange, Conversation,
    GenerationCompletion, GenerationFailureCategory, GenerationToolState, IpcError, IpcErrorCode,
    MAX_ARGUMENT_CORRECTIONS, MAX_CHAT_ASSETS, MAX_RECOVERY_ATTEMPTS, ParsedToolCall, PlanPolicy,
    PromptToolDefinition, RecoveryKind, RequestId, ToolOutcome, calls_available_in_phase,
    conversation_image_count, exact_argument_repair_error, execute_chat_tool_batch,
    inactive_tool_batch_repairable, invalid_tool_batch_repairable, is_forecast_call, is_plan_call,
    is_plan_relationship_error, mixed_read_only_invalid_batch_repairable,
    normalize_artifact_arguments, persist_tool_exchanges,
    plan_prefixed_invalid_operation_batch_repairable, reject_final_response_tool_batch,
    reject_inactive_tool_batch, reject_invalid_tool_batch, reject_plan_deviation_batch,
    reject_unknown_tool_batch, remaining_plan_forecast_calls, terminal_error,
    trace_plan_tool_results, trace_tool_exchanges, trace_tool_rejection,
    unknown_tool_batch_repairable, uses_dedicated_forecast_dispatch,
    uses_dedicated_stage_completion_dispatch, uses_dedicated_tool_search_dispatch,
    validated_chat_tool_request,
};

pub(super) fn exhausted_provider_recovery_with_cause(
    terminal: ChatTerminal,
    last_error: &IpcError,
) -> ChatTerminal {
    match terminal {
        ChatTerminal::Error {
            error: mut exhausted,
            category,
        } => {
            exhausted.message = format!(
                "Generation failed after {MAX_RECOVERY_ATTEMPTS} recovery attempts. Last error: {}",
                last_error.message
            );
            ChatTerminal::Error {
                error: exhausted,
                category,
            }
        }
        other => other,
    }
}

pub(super) fn file_content_mutation_completed(
    exchanges: &[Result<ChatToolExchange, IpcError>],
) -> bool {
    exchanges
        .iter()
        .filter_map(|exchange| exchange.as_ref().ok())
        .any(|exchange| {
            matches!(
                exchange.request.name,
                ipc_types::ToolName::FilesystemWriteText
                    | ipc_types::ToolName::FilesystemReplaceText
                    | ipc_types::ToolName::FilesystemWriteDelimitedTable
                    | ipc_types::ToolName::OrchestrationTransformArtifact
            ) && matches!(exchange.result.outcome, ToolOutcome::Completed { .. })
        })
}

#[allow(clippy::too_many_arguments)]
#[expect(
    clippy::too_many_lines,
    reason = "phase admission keeps control dispatch and recoverable workspace validation in one ordered boundary"
)]
pub(super) fn validate_or_recover_phase_calls(
    app: &tauri::AppHandle,
    core: &AppCore,
    request_id: &RequestId,
    sequence: &mut u32,
    conversation: &mut Conversation,
    workspaces: &[crate::chat_tools::ChatToolWorkspace],
    tools: &[PromptToolDefinition],
    all_tools: &[PromptToolDefinition],
    tool_state: &mut GenerationToolState,
    generated: &mut ChatGenerationRound,
    round: usize,
    cancellation_requested: &std::sync::Arc<AtomicBool>,
) -> Result<bool, ChatTerminal> {
    if super::plan_pending::defer_preplan_calls(
        app,
        core,
        request_id,
        sequence,
        conversation,
        tool_state,
        generated,
        round,
        cancellation_requested,
    )? {
        return Ok(true);
    }
    normalize_stale_plan_control(core, request_id, conversation, tool_state, generated, round);
    tool_state
        .plan_policy
        .bind_artifact_evidence(&mut generated.calls);
    for call in &mut generated.calls {
        if call.name == "orchestration.transform_artifact" {
            normalize_artifact_arguments(call);
        }
    }
    let mut validation_tools =
        prospective_plan_tools(tools, all_tools, tool_state, &generated.calls);
    tool_state.filter_unavailable_tools(&mut validation_tools);
    if !calls_available_in_phase(&validation_tools, &generated.calls)
        && tool_state
            .plan_policy
            .authorize_safe_read_recovery(&generated.calls, all_tools)
    {
        validation_tools = tool_state.projected_tools(all_tools);
        #[cfg(debug_assertions)]
        core.record_development_tool_trace(&serde_json::json!({
            "event": "plan_scope_recovered",
            "requestId": request_id.0,
            "conversationId": conversation.id.0,
            "round": round,
            "tools": generated.calls.iter().map(|call| call.name.as_str()).collect::<Vec<_>>()
        }));
    }
    let (calls_available, recoverable) =
        phase_call_availability(&validation_tools, tool_state, &generated.calls);
    if !calls_available {
        if final_response_tool_batch_repairable(tool_state, &generated.calls) {
            return recover_final_response_tool_call(
                app,
                core,
                request_id,
                sequence,
                conversation,
                tool_state,
                generated,
                round,
            );
        }
        if let Some(recovered) = recover_unknown_tool_call(
            app,
            core,
            request_id,
            sequence,
            conversation,
            &validation_tools,
            tool_state,
            generated,
            round,
        )? {
            return Ok(recovered);
        }
        if recoverable {
            return recover_inactive_tool_call(
                app,
                core,
                request_id,
                sequence,
                conversation,
                &validation_tools,
                tool_state,
                generated,
                round,
            );
        }
        trace_tool_rejection(
            core,
            request_id,
            conversation,
            round,
            &generated.calls,
            "orchestration_phase",
            "The requested tool is unavailable in the current orchestration phase",
        );
        return Err(terminal_error(
            tool_preparation_terminal_error(),
            GenerationFailureCategory::OrchestrationPhase,
        ));
    }
    if let Some(recovered) = recover_invalid_tool_arguments(
        app,
        core,
        request_id,
        sequence,
        conversation,
        workspaces,
        &validation_tools,
        all_tools,
        tool_state,
        generated,
        round,
        cancellation_requested,
    )? {
        return Ok(recovered);
    }
    // Forecast control is validated and installed by its dedicated dispatcher.
    // Treating it as semantic task work here rejects it as unrelated before the
    // accepted-plan/forecast boundary can be crossed.
    if uses_dedicated_forecast_dispatch(&generated.calls) {
        tool_state.record_calls(&generated.calls);
        return Ok(false);
    }
    if uses_dedicated_stage_completion_dispatch(&generated.calls) {
        tool_state.record_calls(&generated.calls);
        return Ok(false);
    }
    if generated
        .calls
        .iter()
        .any(|call| call.name == "orchestration.recall")
        || uses_dedicated_tool_search_dispatch(&generated.calls)
    {
        tool_state.record_calls(&generated.calls);
        return Ok(false);
    }
    if !tool_state.plan_policy.active() && is_direct_discovery_call_batch(&generated.calls) {
        // Bounded web retrieval and resource metadata discovery retain normal
        // schema, permission and broker validation without inventing semantic
        // stage state solely to permit information gathering.
        tool_state.record_calls(&generated.calls);
        return Ok(false);
    }
    if let Some(recovered) = recover_plan_policy_calls(
        app,
        core,
        request_id,
        sequence,
        conversation,
        tool_state,
        generated,
        round,
    )? {
        return Ok(recovered);
    }
    tool_state.record_calls(&generated.calls);
    Ok(false)
}

pub(super) fn is_direct_discovery_call_batch(calls: &[ParsedToolCall]) -> bool {
    !calls.is_empty()
        && calls.iter().all(|call| {
            if crate::chat_tools::is_discovery_call(call) {
                return true;
            }
            matches!(
                call.name.as_str(),
                "web.search" | "web.retrieve" | "filesystem.list" | "filesystem.find_paths"
            )
        })
}

pub(super) fn final_response_tool_batch_repairable(
    tool_state: &GenerationToolState,
    calls: &[ParsedToolCall],
) -> bool {
    tool_state.plan_policy.final_response_ready() && inactive_tool_batch_repairable(calls)
}

pub(super) fn normalize_stale_plan_control(
    core: &AppCore,
    request_id: &RequestId,
    conversation: &Conversation,
    tool_state: &GenerationToolState,
    generated: &mut ChatGenerationRound,
    round: usize,
) {
    let Some(plan) = generated
        .calls
        .first_mut()
        .filter(|call| is_plan_call(call))
    else {
        return;
    };
    if !tool_state.plan_policy.normalize_stale_initial(plan) {
        return;
    }
    #[cfg(not(debug_assertions))]
    let _ = (core, request_id, conversation, round);
    #[cfg(debug_assertions)]
    core.record_development_tool_trace(&serde_json::json!({
        "event": "stale_plan_control_normalized",
        "requestId": request_id.0,
        "conversationId": conversation.id.0,
        "round": round,
        "from": "initial",
        "to": "revise"
    }));
}

#[allow(clippy::too_many_arguments)]
pub(super) fn recover_inactive_tool_call(
    app: &tauri::AppHandle,
    core: &AppCore,
    request_id: &RequestId,
    sequence: &mut u32,
    conversation: &mut Conversation,
    tools: &[PromptToolDefinition],
    tool_state: &mut GenerationToolState,
    generated: &mut ChatGenerationRound,
    round: usize,
) -> Result<bool, ChatTerminal> {
    tool_state.suspend_bounded_stage();
    let available_tools = tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect::<Vec<_>>();
    let kind = RecoveryKind::StalePhase;
    let guidance = if !tool_state.recall_available()
        && generated
            .calls
            .iter()
            .any(|call| call.name == "orchestration.recall")
        && !tool_state.plan_policy.active()
    {
        "This fresh conversation has no earlier inputs or observations to recall. Use the current visible user input directly. Submit the plan alone for multi-stage work, or use the advertised direct-web route for a bounded web answer."
    } else if tool_state.plan_policy.active() {
        "The requested operations are not active in this phase. Resume the same plan step using advertised operations, or revise only for a concrete missing dependency."
    } else {
        "The requested control is not active before planning. Submit orchestration.submit_plan alone before attempting task-stage controls."
    };
    tool_state.request_recovery(
        kind,
        tool_state.plan_policy.correction_scope(),
        format!(
            "calls={};active={}",
            tool_call_fingerprint(&generated.calls),
            available_tools.join(",")
        ),
        guidance,
    )?;
    trace_tool_rejection(
        core,
        request_id,
        conversation,
        round,
        &generated.calls,
        "orchestration_phase",
        "The requested tool is not active in the current orchestration phase",
    );
    tool_state.record_calls(&generated.calls);
    let scope = tool_state.plan_policy.correction_scope();
    let corrections = tool_state.phase_correction_scopes.entry(scope).or_default();
    *corrections = corrections.saturating_add(1);
    let exchanges = reject_inactive_tool_batch(
        app,
        request_id,
        sequence,
        conversation,
        &generated.calls,
        &available_tools,
    );
    trace_and_persist_recovery(
        core,
        request_id,
        conversation,
        round,
        &tool_state.plan_policy,
        generated,
        exchanges,
    )?;
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn recover_final_response_tool_call(
    app: &tauri::AppHandle,
    core: &AppCore,
    request_id: &RequestId,
    sequence: &mut u32,
    conversation: &mut Conversation,
    tool_state: &mut GenerationToolState,
    generated: &mut ChatGenerationRound,
    round: usize,
) -> Result<bool, ChatTerminal> {
    const GUIDANCE: &str = "The verified outcome is complete and the tool surface is intentionally closed. Do not call any task or orchestration tool, revise the plan, or search for tools. Respond to the user now in plain text using the established result retained in context.";

    tool_state.request_recovery(
        RecoveryKind::StalePhase,
        tool_state.plan_policy.correction_scope(),
        format!("final_response:{}", tool_call_fingerprint(&generated.calls)),
        GUIDANCE,
    )?;
    trace_tool_rejection(
        core,
        request_id,
        conversation,
        round,
        &generated.calls,
        "final_response",
        GUIDANCE,
    );
    tool_state.record_calls(&generated.calls);
    let exchanges =
        reject_final_response_tool_batch(app, request_id, sequence, conversation, &generated.calls);
    trace_and_persist_recovery(
        core,
        request_id,
        conversation,
        round,
        &tool_state.plan_policy,
        generated,
        exchanges,
    )?;
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
#[expect(
    clippy::too_many_lines,
    reason = "recovery keeps one auditable validation, execution, and persistence sequence"
)]
pub(super) fn recover_invalid_tool_arguments(
    app: &tauri::AppHandle,
    core: &AppCore,
    request_id: &RequestId,
    sequence: &mut u32,
    conversation: &mut Conversation,
    workspaces: &[crate::chat_tools::ChatToolWorkspace],
    validation_tools: &[PromptToolDefinition],
    all_tools: &[PromptToolDefinition],
    tool_state: &mut GenerationToolState,
    generated: &mut ChatGenerationRound,
    round: usize,
    cancellation_requested: &std::sync::Arc<AtomicBool>,
) -> Result<Option<bool>, ChatTerminal> {
    let invalid = match tool_state
        .path_references
        .resolve_calls(&mut generated.calls)
    {
        Ok(()) => generated
            .calls
            .iter()
            .enumerate()
            .filter_map(|(index, call)| {
                validated_chat_tool_request(call, workspaces)
                    .err()
                    .map(|error| (index, error))
            })
            .collect::<Vec<_>>(),
        Err(error) => vec![error],
    };
    let Some((invalid_index, base_error)) = invalid.first() else {
        return Ok(None);
    };
    let error = exact_argument_repair_error(
        &generated.calls[*invalid_index],
        validation_tools,
        base_error,
    );
    tool_state.suspend_bounded_stage();
    trace_tool_rejection(
        core,
        request_id,
        conversation,
        round,
        &generated.calls,
        "tool_validation",
        &error.message,
    );
    let mixed_read_only_repair = invalid.len() == 1
        && mixed_read_only_argument_repair_allowed(tool_state, &generated.calls, *invalid_index);
    let plan_prefixed_operation_repair = invalid.len() == 1
        && valid_plan_prefixed_operation_repair_allowed(
            tool_state,
            &generated.calls,
            *invalid_index,
            all_tools,
            &error,
        );
    if plan_prefixed_operation_repair {
        return Ok(None);
    }
    if invalid_tool_batch_repairable(&generated.calls) || mixed_read_only_repair {
        tool_state.request_recovery(
            RecoveryKind::InvalidArguments,
            tool_state.plan_policy.correction_scope(),
            argument_correction_fingerprint(&generated.calls, &error),
            format!(
                "The `{}` call failed argument validation: {} Correct only this call using its advertised closed schema; retained plan and evidence remain valid.",
                generated.calls[*invalid_index].name, error.message
            ),
        )?;
    }
    if !tool_argument_repair_allowed(tool_state, &generated.calls, &error)
        && !mixed_read_only_repair
    {
        return Err(terminal_error(
            tool_preparation_terminal_error(),
            GenerationFailureCategory::ToolValidation,
        ));
    }
    if mixed_read_only_repair
        && tool_state
            .plan_policy
            .validate_calls(&generated.calls)
            .is_err()
    {
        return Ok(None);
    }
    tool_state.record_calls(&generated.calls);
    record_argument_correction(tool_state, &generated.calls, &error);
    if mixed_read_only_repair {
        let valid_calls = generated
            .calls
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != *invalid_index)
            .map(|(_, call)| call.clone())
            .collect::<Vec<_>>();
        let mut remaining_image_slots =
            MAX_CHAT_ASSETS.saturating_sub(conversation_image_count(conversation));
        let mut valid_exchanges = execute_chat_tool_batch(
            app,
            core,
            request_id,
            sequence,
            conversation,
            workspaces,
            &valid_calls,
            &tool_state.operation_ledger,
            &mut tool_state.path_references,
            &mut remaining_image_slots,
            cancellation_requested,
        )
        .into_iter();
        let mut correction = reject_invalid_tool_batch(
            app,
            request_id,
            sequence,
            conversation,
            std::slice::from_ref(&generated.calls[*invalid_index]),
            &error,
        )
        .into_iter();
        let exchanges = generated
            .calls
            .iter()
            .enumerate()
            .map(|(index, _)| {
                if index == *invalid_index {
                    correction
                        .next()
                        .unwrap_or_else(|| Err(IpcError::internal()))
                } else {
                    valid_exchanges
                        .next()
                        .unwrap_or_else(|| Err(IpcError::internal()))
                }
            })
            .collect::<Vec<_>>();
        trace_tool_exchanges(core, request_id, conversation, round, &exchanges);
        tool_state.trace_and_record_results(
            core,
            request_id,
            conversation,
            round,
            &generated.calls,
            &exchanges,
        );
        persist_recovery_exchange(core, request_id, conversation, generated, exchanges)?;
        return Ok(Some(true));
    }
    let exchanges = reject_invalid_tool_batch(
        app,
        request_id,
        sequence,
        conversation,
        &generated.calls,
        &error,
    );
    trace_and_persist_recovery(
        core,
        request_id,
        conversation,
        round,
        &tool_state.plan_policy,
        generated,
        exchanges,
    )?;
    Ok(Some(true))
}

pub(super) fn phase_call_availability(
    tools: &[PromptToolDefinition],
    _tool_state: &GenerationToolState,
    calls: &[ParsedToolCall],
) -> (bool, bool) {
    let available = calls_available_in_phase(tools, calls);
    let recoverable = !available
        && (inactive_tool_batch_repairable(calls) || unknown_tool_batch_repairable(calls));
    (available, recoverable)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn recover_unknown_tool_call(
    app: &tauri::AppHandle,
    core: &AppCore,
    request_id: &RequestId,
    sequence: &mut u32,
    conversation: &mut Conversation,
    tools: &[PromptToolDefinition],
    tool_state: &mut GenerationToolState,
    generated: &mut ChatGenerationRound,
    round: usize,
) -> Result<Option<bool>, ChatTerminal> {
    if !unknown_tool_batch_repairable(&generated.calls) {
        return Ok(None);
    }
    tool_state.suspend_bounded_stage();
    let available_tools = tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect::<Vec<_>>();
    tool_state.request_recovery(
        RecoveryKind::UnavailableTool,
        tool_state.plan_policy.correction_scope(),
        format!(
            "calls={};registry={}",
            tool_call_fingerprint(&generated.calls),
            available_tools.join(",")
        ),
        "The requested tool is not in the active registry. Resume the same step with an advertised tool; do not invent a replacement or restart discovery.",
    )?;
    trace_tool_rejection(
        core,
        request_id,
        conversation,
        round,
        &generated.calls,
        "tool_registry",
        "The requested tool is not in the active registry",
    );
    tool_state.record_calls(&generated.calls);
    tool_state.registry_corrections = tool_state.registry_corrections.saturating_add(1);
    let exchanges = reject_unknown_tool_batch(
        app,
        request_id,
        sequence,
        conversation,
        &generated.calls,
        &available_tools,
    );
    trace_and_persist_recovery(
        core,
        request_id,
        conversation,
        round,
        &tool_state.plan_policy,
        generated,
        exchanges,
    )?;
    Ok(Some(true))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn recover_plan_policy_calls(
    app: &tauri::AppHandle,
    core: &AppCore,
    request_id: &RequestId,
    sequence: &mut u32,
    conversation: &mut Conversation,
    tool_state: &mut GenerationToolState,
    generated: &mut ChatGenerationRound,
    round: usize,
) -> Result<Option<bool>, ChatTerminal> {
    let Err(error) = tool_state.plan_policy.validate_calls(&generated.calls) else {
        return Ok(None);
    };
    trace_tool_rejection(
        core,
        request_id,
        conversation,
        round,
        &generated.calls,
        "orchestration_plan",
        &error.message,
    );
    if tool_state.plan_policy.correction_available(&error) {
        tool_state.request_recovery(
            RecoveryKind::PlanValidation,
            tool_state.plan_policy.correction_scope(),
            format!(
                "{}:{:?}",
                tool_call_fingerprint(&generated.calls),
                error.code
            ),
            format!(
                "The current batch did not satisfy the accepted plan relationship: {} Resume the same semantic step and change only the rejected operation or dependency relationship.",
                error.message
            ),
        )?;
    }
    if tool_state.plan_policy.correction_available(&error) {
        tool_state.record_calls(&generated.calls);
        tool_state.plan_policy.record_correction(&error);
        let pre_execution_deviation = is_plan_relationship_error(&error)
            || tool_state
                .plan_policy
                .pre_execution_evidence_deviation(&generated.calls)
                .is_some();
        let exchanges = if pre_execution_deviation {
            reject_plan_deviation_batch(
                app,
                request_id,
                sequence,
                conversation,
                &generated.calls,
                &error,
            )
        } else {
            reject_invalid_tool_batch(
                app,
                request_id,
                sequence,
                conversation,
                &generated.calls,
                &error,
            )
        };
        trace_and_persist_recovery(
            core,
            request_id,
            conversation,
            round,
            &tool_state.plan_policy,
            generated,
            exchanges,
        )?;
        return Ok(Some(true));
    }
    Err(terminal_error(
        tool_preparation_terminal_error(),
        GenerationFailureCategory::OrchestrationPhase,
    ))
}

pub(super) fn tool_argument_repair_allowed(
    tool_state: &GenerationToolState,
    calls: &[ParsedToolCall],
    error: &IpcError,
) -> bool {
    let _ = error;
    tool_state.argument_corrections < MAX_ARGUMENT_CORRECTIONS
        && invalid_tool_batch_repairable(calls)
}

pub(super) fn mixed_read_only_argument_repair_allowed(
    tool_state: &GenerationToolState,
    calls: &[ParsedToolCall],
    invalid_index: usize,
) -> bool {
    tool_state.argument_corrections < MAX_ARGUMENT_CORRECTIONS
        && mixed_read_only_invalid_batch_repairable(calls, invalid_index)
}

pub(super) fn record_argument_correction(
    tool_state: &mut GenerationToolState,
    calls: &[ParsedToolCall],
    error: &IpcError,
) {
    tool_state.argument_corrections = tool_state.argument_corrections.saturating_add(1);
    let _ = (calls, error);
}

pub(super) fn argument_correction_fingerprint(
    calls: &[ParsedToolCall],
    error: &IpcError,
) -> String {
    format!("{}:{:?}", tool_call_fingerprint(calls), error.code)
}

pub(super) fn tool_call_fingerprint(calls: &[ParsedToolCall]) -> String {
    serde_json::to_string(
        &calls
            .iter()
            .map(|call| (&call.name, &call.arguments))
            .collect::<Vec<_>>(),
    )
    .unwrap_or_default()
}

pub(super) fn valid_plan_prefixed_operation_repair_allowed(
    tool_state: &GenerationToolState,
    calls: &[ParsedToolCall],
    invalid_index: usize,
    tools: &[PromptToolDefinition],
    error: &IpcError,
) -> bool {
    if !plan_prefixed_invalid_operation_batch_repairable(calls, invalid_index)
        || !tool_argument_repair_allowed(
            tool_state,
            std::slice::from_ref(&calls[invalid_index]),
            error,
        )
    {
        return false;
    }
    let mut preview = tool_state.plan_policy.clone();
    let remaining = remaining_plan_forecast_calls(tool_state.total_calls, calls.len());
    preview.accept(&calls[0], tools, remaining).is_ok()
}

pub(super) fn prospective_plan_tools(
    current_tools: &[PromptToolDefinition],
    all_tools: &[PromptToolDefinition],
    tool_state: &GenerationToolState,
    calls: &[ParsedToolCall],
) -> Vec<PromptToolDefinition> {
    if let Some(forecast) = calls.first().filter(|call| is_forecast_call(call)) {
        let mut preview = tool_state.plan_policy.clone();
        if preview.accept_forecast(forecast, all_tools).is_ok() {
            return preview.tools(all_tools);
        }
        return all_tools.to_vec();
    }
    let Some(plan) = calls.first().filter(|call| is_plan_call(call)) else {
        return current_tools.to_vec();
    };
    let mut preview = tool_state.plan_policy.clone();
    let remaining = remaining_plan_forecast_calls(tool_state.total_calls, calls.len());
    if preview.accept(plan, all_tools, remaining).is_ok() {
        if preview.forecast_required() {
            return all_tools.to_vec();
        }
        let mut prospective = preview.tools(all_tools);
        if !prospective.iter().any(|tool| tool.name == plan.name)
            && let Some(control) = all_tools.iter().find(|tool| tool.name == plan.name)
        {
            prospective.push(control.clone());
        }
        prospective
    } else {
        all_tools.to_vec()
    }
}

pub(super) fn tool_preparation_terminal_error() -> IpcError {
    IpcError {
        code: IpcErrorCode::Unsupported,
        message: "Minutus could not safely repair the emitted workspace batch because it was not one known, bounded operation in the current capability catalogue. Continue by choosing an advertised operation, materially revising the plan around an unavailable capability, or clarifying the required outcome."
            .to_owned(),
        retryable: true,
    }
}

pub(super) fn persist_recovery_exchange(
    core: &AppCore,
    request_id: &RequestId,
    conversation: &mut Conversation,
    generated: &mut ChatGenerationRound,
    exchanges: Vec<Result<ChatToolExchange, IpcError>>,
) -> Result<(), ChatTerminal> {
    persist_tool_exchanges(
        core,
        request_id,
        conversation,
        std::mem::take(&mut generated.reasoning),
        std::mem::take(&mut generated.content),
        exchanges,
        &generated.image_ids,
        &generated.binding_sha256,
    )
    .map_err(|error| terminal_error(error, GenerationFailureCategory::Persistence))
}

pub(super) fn trace_and_persist_recovery(
    core: &AppCore,
    request_id: &RequestId,
    conversation: &mut Conversation,
    round: usize,
    plan: &PlanPolicy,
    generated: &mut ChatGenerationRound,
    exchanges: Vec<Result<ChatToolExchange, IpcError>>,
) -> Result<(), ChatTerminal> {
    trace_tool_exchanges(core, request_id, conversation, round, &exchanges);
    trace_plan_tool_results(
        core,
        request_id,
        conversation,
        round,
        plan,
        &generated.calls,
        &exchanges,
    );
    persist_recovery_exchange(core, request_id, conversation, generated, exchanges)
}

pub(super) const fn completion_reason_name(reason: GenerationCompletion) -> &'static str {
    match reason {
        GenerationCompletion::Stop => "stop",
        GenerationCompletion::Length => "length",
    }
}

#[cfg(test)]
mod provider_recovery_tests {
    use super::*;

    #[test]
    fn exhausted_recovery_preserves_the_last_sanitized_failure()
    -> Result<(), Box<dyn std::error::Error>> {
        let last_error = stream_error(ModelOutputFailureKind::UnexpectedEnd);
        let terminal = terminal_error(
            IpcError::internal(),
            GenerationFailureCategory::OrchestrationPhase,
        );
        let ChatTerminal::Error { error, .. } =
            exhausted_provider_recovery_with_cause(terminal, &last_error)
        else {
            return Err("expected terminal error".into());
        };
        assert!(error.message.contains("after 3 recovery attempts"));
        assert!(error.message.contains("ended before completion"));
        assert!(!error.message.contains("service is responsive"));
        Ok(())
    }
}
