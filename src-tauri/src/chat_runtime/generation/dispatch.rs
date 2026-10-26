//! Dispatch for workspace, orchestration, forecast, and stage-control calls.

use super::*;

mod plan_control;
pub(super) mod recall;

#[cfg(test)]
pub(in crate::chat_runtime::generation) use plan_control::current_reads_can_survive_rejected_plan;
pub(in crate::chat_runtime::generation) use plan_control::{
    accepted_plan_task_deferral, admit_plan_control,
};
use plan_control::{
    complete_plan_control, correct_invalid_operation_after_accepted_plan,
    execute_advisory_task_segment, finish_plan_prefixed_round, finish_positioned_stage_round,
    persist_executed_tool_batch, persist_unfinished_tool_batch, recover_rejected_plan_admission,
    reject_plan_prefixed_operation_batch, request_stage_completion_recovery,
};
pub(in crate::chat_runtime::generation) use plan_control::{
    is_plan_relationship_error, should_schedule_forecast_recovery,
    should_schedule_outcome_recovery, stage_completion_boundary, terminal_without_tool_calls,
};

#[expect(
    clippy::too_many_arguments,
    reason = "tool execution binds independent trusted request components at one boundary"
)]
pub(super) fn execute_workspace_tool_round(
    app: &tauri::AppHandle,
    core: &AppCore,
    request_id: &RequestId,
    sequence: &mut u32,
    conversation: &mut Conversation,
    workspaces: &[crate::chat_tools::ChatToolWorkspace],
    tool_state: &mut GenerationToolState,
    telemetry: &mut RequestExecutionTelemetry,
    generated: ChatGenerationRound,
    round: usize,
    cancellation_requested: &AtomicBool,
) -> Option<ChatTerminal> {
    let mut remaining_image_slots =
        MAX_CHAT_ASSETS.saturating_sub(conversation_image_count(conversation));
    let exchanges = execute_chat_tool_batch(
        app,
        core,
        request_id,
        sequence,
        conversation,
        workspaces,
        &generated.calls,
        &tool_state.operation_ledger,
        &mut tool_state.path_references,
        &mut remaining_image_slots,
        cancellation_requested,
    );
    telemetry.record_exchanges(&exchanges);
    trace_tool_exchanges(core, request_id, conversation, round, &exchanges);
    tool_state.trace_and_record_results(
        core,
        request_id,
        conversation,
        round,
        &generated.calls,
        &exchanges,
    );
    tool_state.observe_recovery_checkpoint();
    let transient_recovery =
        schedule_transient_tool_recovery(tool_state, &generated.calls, &exchanges).err();
    let allow_automatic_completion = tool_state.plan_policy.allows_automatic_write_confirmation();
    let persisted = persist_executed_tool_batch(
        core,
        request_id,
        conversation,
        generated,
        exchanges,
        allow_automatic_completion,
    );
    persisted.or(transient_recovery)
}

pub(super) fn schedule_transient_tool_recovery(
    tool_state: &mut GenerationToolState,
    calls: &[ParsedToolCall],
    exchanges: &[Result<ChatToolExchange, IpcError>],
) -> Result<(), ChatTerminal> {
    let Some((call, error)) = calls.iter().zip(exchanges).find_map(|(call, exchange)| {
        let exchange = exchange.as_ref().ok()?;
        let ToolOutcome::Failed { error } = &exchange.result.outcome else {
            return None;
        };
        error.retryable.then_some((call, error))
    }) else {
        return Ok(());
    };
    tool_state.request_transient_recovery(
        RecoveryKind::TransientTool,
        tool_state.plan_policy.correction_scope(),
        tool_call_fingerprint(std::slice::from_ref(call)),
        format!(
            "The `{}` operation failed transiently: {} Resume this same semantic step; retain all completed evidence and retry only the failed operation or use a supported same-step alternative.",
            call.name, error.message
        ),
    )
}

pub(super) fn is_plan_call(call: &ParsedToolCall) -> bool {
    call.name == "orchestration.submit_plan"
}

pub(super) fn is_forecast_call(call: &ParsedToolCall) -> bool {
    call.name == FORECAST_TOOL
}

pub(super) fn is_stage_completion_call(call: &ParsedToolCall) -> bool {
    call.name == COMPLETE_STAGE_TOOL
}

pub(super) fn is_tool_search_call(call: &ParsedToolCall) -> bool {
    call.name == SEARCH_TOOLS_TOOL
}

pub(super) fn uses_dedicated_forecast_dispatch(calls: &[ParsedToolCall]) -> bool {
    calls.first().is_some_and(is_forecast_call)
}

pub(super) fn uses_dedicated_stage_completion_dispatch(calls: &[ParsedToolCall]) -> bool {
    calls.iter().any(is_stage_completion_call)
}

pub(super) fn uses_dedicated_tool_search_dispatch(calls: &[ParsedToolCall]) -> bool {
    calls.iter().any(is_tool_search_call)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn execute_tool_search_round(
    app: &tauri::AppHandle,
    core: &AppCore,
    request_id: &RequestId,
    sequence: &mut u32,
    conversation: &mut Conversation,
    all_tools: &[PromptToolDefinition],
    tool_state: &mut GenerationToolState,
    telemetry: &mut RequestExecutionTelemetry,
    generated: ChatGenerationRound,
    round: usize,
) -> Option<ChatTerminal> {
    let [call] = generated.calls.as_slice() else {
        let error = IpcError::scope_correction(
            "orchestration.search_tools must be called alone. Its matching schemas become directly callable on the next round; continue the same stage without revising the plan.",
        );
        if let Err(terminal) = tool_state.request_recovery(
            RecoveryKind::PlanValidation,
            tool_state.plan_policy.correction_scope(),
            tool_call_fingerprint(&generated.calls),
            error.message.clone(),
        ) {
            return Some(terminal);
        }
        let exchanges = reject_plan_deviation_batch(
            app,
            request_id,
            sequence,
            conversation,
            &generated.calls,
            &error,
        );
        telemetry.record_exchanges(&exchanges);
        trace_tool_exchanges(core, request_id, conversation, round, &exchanges);
        return persist_unfinished_tool_batch(core, request_id, conversation, generated, exchanges);
    };
    let mut searchable_tools = all_tools.to_vec();
    tool_state.filter_unavailable_tools(&mut searchable_tools);
    let outcome = match search_tool_definitions(call, &searchable_tools) {
        Ok(outcome) => outcome,
        Err(error) => {
            let exchanges = reject_invalid_tool_batch(
                app,
                request_id,
                sequence,
                conversation,
                std::slice::from_ref(call),
                &error,
            );
            telemetry.record_exchanges(&exchanges);
            trace_tool_exchanges(core, request_id, conversation, round, &exchanges);
            return persist_unfinished_tool_batch(
                core,
                request_id,
                conversation,
                generated,
                exchanges,
            );
        }
    };
    tool_state
        .plan_policy
        .load_task_tools(outcome.loaded_names.clone());
    let exchange = complete_control_call(
        app,
        request_id,
        sequence,
        conversation,
        call,
        &outcome.value,
    )
    .map_or_else(Err, Ok);
    let exchanges = vec![exchange];
    telemetry.record_exchanges(&exchanges);
    trace_tool_exchanges(core, request_id, conversation, round, &exchanges);
    persist_unfinished_tool_batch(core, request_id, conversation, generated, exchanges)
}

/// Ollama does not currently expose a reliable exact `tool_choice` control.
/// The strongest provider-neutral routing that preserves a legal escape is an
/// active registry containing the required operation and plan revision only;
/// ordinary assistant output remains available for `ask_user`.
pub(super) fn bounded_stage_tools(
    tools: Vec<PromptToolDefinition>,
    contract: Option<&BoundedStageContract>,
) -> Vec<PromptToolDefinition> {
    let Some(contract) = contract else {
        return tools;
    };
    tools
        .into_iter()
        .filter(|tool| {
            tool.name == contract.operation() || tool.name == "orchestration.submit_plan"
        })
        .collect()
}

pub(super) fn bounded_stage_asks_user(
    generated: &ChatGenerationRound,
    contract: Option<&BoundedStageContract>,
) -> bool {
    contract.is_some()
        && generated.calls.is_empty()
        && generated.content.trim().len() <= 1_000
        && generated.content.contains('?')
}

#[allow(clippy::too_many_arguments)]
#[expect(
    clippy::too_many_lines,
    reason = "one plan-prefixed batch must preserve a single admission, correction, execution and persistence boundary"
)]
pub(super) fn execute_plan_prefixed_round(
    app: &tauri::AppHandle,
    core: &AppCore,
    request_id: &RequestId,
    sequence: &mut u32,
    conversation: &mut Conversation,
    workspaces: &[crate::chat_tools::ChatToolWorkspace],
    all_tools: &[PromptToolDefinition],
    tool_state: &mut GenerationToolState,
    telemetry: &mut RequestExecutionTelemetry,
    generated: ChatGenerationRound,
    round: usize,
    cancellation_requested: &AtomicBool,
) -> Option<ChatTerminal> {
    let call = &generated.calls[0];
    let value = admit_plan_control(tool_state, call, all_tools, generated.calls.len());
    let value = match value {
        Ok(value) => value,
        Err(error) => {
            return recover_rejected_plan_admission(
                app,
                core,
                request_id,
                sequence,
                conversation,
                workspaces,
                tool_state,
                telemetry,
                round,
                cancellation_requested,
                &error,
                generated,
            );
        }
    };
    if let Err(terminal) = super::workspace_access::require_planned_write_access(
        tool_state,
        super::tool_capabilities(all_tools).0,
    ) {
        return Some(terminal);
    }
    // A semantic-plan escape from the direct web route must not forget the
    // evidence that motivated or supports the expanded work.
    tool_state.retain_preplan_results();
    let exchange =
        match complete_plan_control(app, request_id, sequence, conversation, call, &value) {
            Ok(exchange) => exchange,
            Err(terminal) => return Some(terminal),
        };
    let mut exchanges = vec![Ok(exchange)];
    let operation_calls = &generated.calls[1..];
    if tool_state.plan_policy.requires_plan_only_round() && !operation_calls.is_empty() {
        let error = match accepted_plan_task_deferral(tool_state, operation_calls) {
            Ok(error) => error,
            Err(terminal) => return Some(terminal),
        };
        exchanges.extend(reject_plan_deviation_batch(
            app,
            request_id,
            sequence,
            conversation,
            operation_calls,
            &error,
        ));
        return finish_plan_prefixed_round(
            core,
            request_id,
            conversation,
            tool_state,
            telemetry,
            generated,
            exchanges,
            round,
        );
    }
    if !operation_calls.is_empty() {
        let correction = match correct_invalid_operation_after_accepted_plan(
            app,
            request_id,
            sequence,
            conversation,
            workspaces,
            all_tools,
            tool_state,
            &generated.calls,
        ) {
            Ok(correction) => correction,
            Err(terminal) => return Some(terminal),
        };
        if let Some(correction) = correction {
            exchanges.push(correction);
            telemetry.record_exchanges(&exchanges);
            trace_tool_exchanges(core, request_id, conversation, round, &exchanges);
            trace_plan_tool_results(
                core,
                request_id,
                conversation,
                round,
                &tool_state.plan_policy,
                &generated.calls,
                &exchanges,
            );
            return persist_unfinished_tool_batch(
                core,
                request_id,
                conversation,
                generated,
                exchanges,
            );
        }
        if let Err(error) = tool_state.plan_policy.validate_calls(operation_calls) {
            if !tool_state.plan_policy.correction_available(&error) {
                return Some(terminal_error(
                    tool_preparation_terminal_error(),
                    GenerationFailureCategory::OrchestrationPhase,
                ));
            }
            if let Err(terminal) = tool_state.request_recovery(
                RecoveryKind::PlanValidation,
                tool_state.plan_policy.correction_scope(),
                format!(
                    "{}:{:?}",
                    tool_call_fingerprint(operation_calls),
                    error.code
                ),
                format!(
                    "The operation beside the accepted plan did not satisfy the current step: {} Repair only that operation or its declared relationship.",
                    error.message
                ),
            ) {
                return Some(terminal);
            }
            tool_state.plan_policy.record_correction(&error);
            exchanges.extend(reject_plan_prefixed_operation_batch(
                app,
                request_id,
                sequence,
                conversation,
                operation_calls,
                &tool_state.plan_policy,
                &error,
            ));
        } else {
            let mut remaining_image_slots =
                MAX_CHAT_ASSETS.saturating_sub(conversation_image_count(conversation));
            exchanges.extend(execute_chat_tool_batch(
                app,
                core,
                request_id,
                sequence,
                conversation,
                workspaces,
                operation_calls,
                &tool_state.operation_ledger,
                &mut tool_state.path_references,
                &mut remaining_image_slots,
                cancellation_requested,
            ));
        }
    }
    finish_plan_prefixed_round(
        core,
        request_id,
        conversation,
        tool_state,
        telemetry,
        generated,
        exchanges,
        round,
    )
}

#[allow(clippy::too_many_arguments)]
#[expect(
    clippy::too_many_lines,
    reason = "forecast acceptance and its optional initial operation form one ordered boundary"
)]
pub(super) fn execute_forecast_prefixed_round(
    app: &tauri::AppHandle,
    core: &AppCore,
    request_id: &RequestId,
    sequence: &mut u32,
    conversation: &mut Conversation,
    workspaces: &[crate::chat_tools::ChatToolWorkspace],
    all_tools: &[PromptToolDefinition],
    tool_state: &mut GenerationToolState,
    telemetry: &mut RequestExecutionTelemetry,
    generated: ChatGenerationRound,
    round: usize,
    cancellation_requested: &AtomicBool,
) -> Option<ChatTerminal> {
    let call = &generated.calls[0];
    let value = match tool_state.plan_policy.accept_forecast(call, all_tools) {
        Ok(value) => value,
        Err(error) => {
            if let Err(terminal) = tool_state.request_recovery(
                RecoveryKind::ForecastValidation,
                tool_state.plan_policy.forecast_scope(),
                format!("{}:{:?}", tool_call_fingerprint(std::slice::from_ref(call)), error.code),
                format!("The advisory execution forecast was rejected: {} Correct only the first reported schema or coverage defect; the accepted semantic plan remains active.", error.message),
            ) {
                return Some(terminal);
            }
            let exchanges = reject_invalid_tool_batch(
                app,
                request_id,
                sequence,
                conversation,
                &generated.calls,
                &error,
            );
            telemetry.record_exchanges(&exchanges);
            trace_tool_exchanges(core, request_id, conversation, round, &exchanges);
            trace_execution_forecast(
                core,
                request_id,
                conversation,
                round,
                &tool_state.plan_policy,
                "rejected",
            );
            return persist_unfinished_tool_batch(
                core,
                request_id,
                conversation,
                generated,
                exchanges,
            );
        }
    };
    let exchange =
        match complete_plan_control(app, request_id, sequence, conversation, call, &value) {
            Ok(exchange) => exchange,
            Err(terminal) => return Some(terminal),
        };
    let mut exchanges = vec![Ok(exchange)];
    let operation_calls = &generated.calls[1..];
    if !operation_calls.is_empty() {
        if let Err(error) = tool_state.plan_policy.validate_calls(operation_calls) {
            if let Err(terminal) = tool_state.request_recovery(
                RecoveryKind::PlanValidation,
                tool_state.plan_policy.correction_scope(),
                tool_call_fingerprint(operation_calls),
                format!("The operation beside the accepted forecast did not satisfy a dependency-ready semantic step: {}", error.message),
            ) {
                return Some(terminal);
            }
            exchanges.extend(reject_plan_prefixed_operation_batch(
                app,
                request_id,
                sequence,
                conversation,
                operation_calls,
                &tool_state.plan_policy,
                &error,
            ));
        } else {
            let mut remaining_image_slots =
                MAX_CHAT_ASSETS.saturating_sub(conversation_image_count(conversation));
            exchanges.extend(execute_chat_tool_batch(
                app,
                core,
                request_id,
                sequence,
                conversation,
                workspaces,
                operation_calls,
                &tool_state.operation_ledger,
                &mut tool_state.path_references,
                &mut remaining_image_slots,
                cancellation_requested,
            ));
        }
    }
    trace_execution_forecast(
        core,
        request_id,
        conversation,
        round,
        &tool_state.plan_policy,
        "accepted",
    );
    finish_plan_prefixed_round(
        core,
        request_id,
        conversation,
        tool_state,
        telemetry,
        generated,
        exchanges,
        round,
    )
}

#[allow(clippy::too_many_arguments)]
#[expect(
    clippy::too_many_lines,
    reason = "positional stage completion preserves ordered execution, state transitions, recovery, and persistence"
)]
pub(super) fn execute_positioned_stage_completion_round(
    app: &tauri::AppHandle,
    core: &AppCore,
    request_id: &RequestId,
    sequence: &mut u32,
    conversation: &mut Conversation,
    workspaces: &[crate::chat_tools::ChatToolWorkspace],
    tool_state: &mut GenerationToolState,
    telemetry: &mut RequestExecutionTelemetry,
    generated: ChatGenerationRound,
    round: usize,
    cancellation_requested: &AtomicBool,
) -> Option<ChatTerminal> {
    let (completion_index, completion_call) = match stage_completion_boundary(&generated.calls) {
        Ok(boundary) => boundary,
        Err(error) => {
            if let Err(terminal) =
                request_stage_completion_recovery(tool_state, &generated.calls, &error)
            {
                return Some(terminal);
            }
            let exchanges = reject_plan_deviation_batch(
                app,
                request_id,
                sequence,
                conversation,
                &generated.calls,
                &error,
            );
            return finish_positioned_stage_round(
                core,
                request_id,
                conversation,
                telemetry,
                generated,
                exchanges,
                round,
            );
        }
    };
    let completion_call = completion_call.clone();
    let prefix_calls = generated.calls[..completion_index].to_vec();
    let suffix_calls = generated.calls[completion_index.saturating_add(1)..].to_vec();
    let completion_validation = tool_state
        .plan_policy
        .validate_advisory_stage_completion(&completion_call);

    let mut remaining_image_slots =
        MAX_CHAT_ASSETS.saturating_sub(conversation_image_count(conversation));
    let prefix_exchanges = execute_advisory_task_segment(
        app,
        core,
        request_id,
        sequence,
        conversation,
        workspaces,
        tool_state,
        &prefix_calls,
        &mut remaining_image_slots,
        cancellation_requested,
    );
    tool_state.trace_and_record_results(
        core,
        request_id,
        conversation,
        round,
        &prefix_calls,
        &prefix_exchanges,
    );

    let completion_exchange = match completion_validation {
        Ok(_) => match tool_state
            .plan_policy
            .complete_advisory_stage(&completion_call)
            .and_then(|value| {
                complete_control_call(
                    app,
                    request_id,
                    sequence,
                    conversation,
                    &completion_call,
                    &value,
                )
            }) {
            Ok(exchange) => Ok(exchange),
            Err(error) => {
                if let Err(terminal) =
                    request_stage_completion_recovery(tool_state, &generated.calls, &error)
                {
                    return Some(terminal);
                }
                reject_invalid_tool_batch(
                    app,
                    request_id,
                    sequence,
                    conversation,
                    std::slice::from_ref(&completion_call),
                    &error,
                )
                .into_iter()
                .next()
                .unwrap_or_else(|| Err(IpcError::internal()))
            }
        },
        Err(error) => {
            if let Err(terminal) =
                request_stage_completion_recovery(tool_state, &generated.calls, &error)
            {
                return Some(terminal);
            }
            reject_invalid_tool_batch(
                app,
                request_id,
                sequence,
                conversation,
                std::slice::from_ref(&completion_call),
                &error,
            )
            .into_iter()
            .next()
            .unwrap_or_else(|| Err(IpcError::internal()))
        }
    };

    let completion_succeeded = completion_exchange
        .as_ref()
        .is_ok_and(|exchange| matches!(exchange.result.outcome, ToolOutcome::Completed { .. }));
    let suffix_exchanges = if completion_succeeded {
        execute_advisory_task_segment(
            app,
            core,
            request_id,
            sequence,
            conversation,
            workspaces,
            tool_state,
            &suffix_calls,
            &mut remaining_image_slots,
            cancellation_requested,
        )
    } else {
        let error = IpcError::scope_correction(
            "Calls after orchestration.complete_stage were not executed because that stage boundary was rejected; continue from the retained current stage.",
        );
        reject_plan_deviation_batch(
            app,
            request_id,
            sequence,
            conversation,
            &suffix_calls,
            &error,
        )
    };
    tool_state.trace_and_record_results(
        core,
        request_id,
        conversation,
        round,
        &suffix_calls,
        &suffix_exchanges,
    );

    let mut exchanges = prefix_exchanges;
    exchanges.push(completion_exchange);
    exchanges.extend(suffix_exchanges);
    finish_positioned_stage_round(
        core,
        request_id,
        conversation,
        telemetry,
        generated,
        exchanges,
        round,
    )
}
