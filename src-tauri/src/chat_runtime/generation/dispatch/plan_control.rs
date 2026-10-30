use super::*;

#[allow(clippy::too_many_arguments)]
pub(in crate::chat_runtime::generation) fn execute_advisory_task_segment(
    app: &tauri::AppHandle,
    core: &AppCore,
    request_id: &RequestId,
    sequence: &mut u32,
    conversation: &Conversation,
    workspaces: &[crate::chat_tools::ChatToolWorkspace],
    tool_state: &mut GenerationToolState,
    calls: &[ParsedToolCall],
    remaining_image_slots: &mut usize,
    cancellation_requested: &std::sync::Arc<AtomicBool>,
) -> Vec<Result<ChatToolExchange, IpcError>> {
    if calls.is_empty() {
        return Vec::new();
    }
    if let Err(error) = tool_state.plan_policy.validate_calls(calls) {
        return reject_plan_prefixed_operation_batch(
            app,
            request_id,
            sequence,
            conversation,
            calls,
            &tool_state.plan_policy,
            &error,
        );
    }
    execute_chat_tool_batch(
        app,
        core,
        request_id,
        sequence,
        conversation,
        workspaces,
        calls,
        &tool_state.operation_ledger,
        &mut tool_state.path_references,
        remaining_image_slots,
        cancellation_requested,
    )
}

pub(in crate::chat_runtime::generation) fn request_stage_completion_recovery(
    tool_state: &mut GenerationToolState,
    calls: &[ParsedToolCall],
    error: &IpcError,
) -> Result<(), ChatTerminal> {
    tool_state.request_recovery(
        RecoveryKind::PlanValidation,
        tool_state.plan_policy.correction_scope(),
        format!("{}:{:?}", tool_call_fingerprint(calls), error.code),
        format!(
            "The stage completion boundary was rejected: {} Continue the same stage from retained results and emit at most one completion boundary; revise only if the work or dependencies materially changed.",
            error.message
        ),
    )
}

pub(in crate::chat_runtime::generation) fn stage_completion_boundary(
    calls: &[ParsedToolCall],
) -> Result<(usize, &ParsedToolCall), IpcError> {
    let mut completions = calls
        .iter()
        .enumerate()
        .filter(|(_, call)| is_stage_completion_call(call));
    let Some(boundary) = completions.next() else {
        return Err(IpcError::scope_correction(
            "The tool batch does not contain a stage completion boundary",
        ));
    };
    if completions.next().is_some()
        || calls.iter().any(|call| {
            !is_stage_completion_call(call)
                && (is_plan_call(call) || is_forecast_call(call) || is_tool_search_call(call))
        })
    {
        return Err(IpcError::scope_correction(
            "A tool batch may contain exactly one orchestration.complete_stage boundary and no other orchestration control. Task calls before it belong to the current stage; task calls after it belong to the newly ready stage.",
        ));
    }
    Ok(boundary)
}

#[allow(clippy::too_many_arguments)]
pub(in crate::chat_runtime::generation) fn finish_positioned_stage_round(
    core: &AppCore,
    request_id: &RequestId,
    conversation: &mut Conversation,
    telemetry: &mut RequestExecutionTelemetry,
    generated: ChatGenerationRound,
    exchanges: Vec<Result<ChatToolExchange, IpcError>>,
    round: usize,
) -> Option<ChatTerminal> {
    telemetry.record_exchanges(&exchanges);
    trace_tool_exchanges(core, request_id, conversation, round, &exchanges);
    persist_unfinished_tool_batch(core, request_id, conversation, generated, exchanges)
}

pub(in crate::chat_runtime::generation) fn admit_plan_control(
    tool_state: &mut GenerationToolState,
    call: &ParsedToolCall,
    all_tools: &[PromptToolDefinition],
    call_count: usize,
) -> Result<serde_json::Value, IpcError> {
    let accepted = if tool_state.plan_policy.redundant_stale_initial(call) {
        tool_state.plan_policy.redundant_plan_value()
    } else {
        let remaining = remaining_plan_forecast_calls(tool_state.total_calls, call_count);
        tool_state.plan_policy.accept(call, all_tools, remaining)?
    };
    tool_state.plan_pending = None;
    // Resolve the admitted plan before registering any correction for sibling
    // task calls. The next loop must not erase those new, still-relevant errors.
    tool_state.observe_recovery_checkpoint();
    tool_state.recovery.resolve_plan_validation();
    Ok(accepted)
}

pub(in crate::chat_runtime::generation) fn accepted_plan_task_deferral(
    tool_state: &mut GenerationToolState,
    operation_calls: &[ParsedToolCall],
) -> Result<IpcError, ChatTerminal> {
    let plan = &tool_state.plan_policy;
    let error = IpcError::scope_correction(if plan.forecast_required() {
        "Plan accepted and retained. Only the task calls bundled with it were not executed. Do not resubmit the plan. Submit orchestration.submit_forecast next; dependency-ready initial operations may follow that accepted forecast."
    } else if plan.uses_advisory_stages() {
        "Plan accepted and retained. Only the task calls bundled with it were deferred and not executed. Do not resubmit the plan. Emit fresh task calls next using the original inputs and available task schemas; use orchestration.search_tools if another schema is needed. No execution forecast is required. Deferred calls never execute automatically and normal permissions and approvals still apply."
    } else {
        "Plan accepted and retained. Only the task calls bundled with it were not executed. Do not resubmit the plan. Emit a fresh dependency-ready operation next; normal permissions and approvals still apply."
    });
    let forecast_required = plan.forecast_required();
    tool_state.request_recovery(
        if forecast_required {
            RecoveryKind::ForecastValidation
        } else {
            RecoveryKind::StalePhase
        },
        if forecast_required {
            plan.forecast_scope()
        } else {
            plan.correction_scope()
        },
        tool_call_fingerprint(operation_calls),
        error.message.clone(),
    )?;
    Ok(error)
}

pub(in crate::chat_runtime::generation) fn complete_plan_control(
    app: &tauri::AppHandle,
    request_id: &RequestId,
    sequence: &mut u32,
    conversation: &Conversation,
    call: &ParsedToolCall,
    value: &serde_json::Value,
) -> Result<ChatToolExchange, ChatTerminal> {
    complete_control_call(app, request_id, sequence, conversation, call, value)
        .map_err(|error| terminal_error(error, GenerationFailureCategory::ToolValidation))
}

#[allow(clippy::too_many_arguments)]
pub(in crate::chat_runtime::generation) fn recover_rejected_plan_admission(
    app: &tauri::AppHandle,
    core: &AppCore,
    request_id: &RequestId,
    sequence: &mut u32,
    conversation: &mut Conversation,
    workspaces: &[crate::chat_tools::ChatToolWorkspace],
    tool_state: &mut GenerationToolState,
    telemetry: &mut RequestExecutionTelemetry,
    round: usize,
    cancellation_requested: &std::sync::Arc<AtomicBool>,
    error: &IpcError,
    generated: ChatGenerationRound,
) -> Option<ChatTerminal> {
    if let Some(exchanges) = execute_current_reads_beside_rejected_plan(
        app,
        core,
        request_id,
        sequence,
        conversation,
        workspaces,
        tool_state,
        &generated.calls,
        cancellation_requested,
        error,
    ) {
        trace_tool_rejection(
            core,
            request_id,
            conversation,
            round,
            std::slice::from_ref(&generated.calls[0]),
            "stale_plan_control",
            &error.message,
        );
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
    handle_rejected_plan_admission(
        app,
        core,
        request_id,
        sequence,
        conversation,
        tool_state,
        telemetry,
        generated,
        round,
        error,
    )
}

#[allow(clippy::too_many_arguments)]
pub(in crate::chat_runtime::generation) fn execute_current_reads_beside_rejected_plan(
    app: &tauri::AppHandle,
    core: &AppCore,
    request_id: &RequestId,
    sequence: &mut u32,
    conversation: &Conversation,
    workspaces: &[crate::chat_tools::ChatToolWorkspace],
    tool_state: &mut GenerationToolState,
    calls: &[ParsedToolCall],
    cancellation_requested: &std::sync::Arc<AtomicBool>,
    plan_error: &IpcError,
) -> Option<Vec<Result<ChatToolExchange, IpcError>>> {
    let [plan, operations @ ..] = calls else {
        return None;
    };
    if !current_reads_can_survive_rejected_plan(workspaces, &mut tool_state.plan_policy, operations)
    {
        return None;
    }
    if tool_state.plan_policy.correction_available(plan_error) {
        tool_state.plan_policy.record_correction(plan_error);
    }
    let mut exchanges = reject_invalid_tool_batch(
        app,
        request_id,
        sequence,
        conversation,
        std::slice::from_ref(plan),
        plan_error,
    );
    let mut remaining_image_slots =
        MAX_CHAT_ASSETS.saturating_sub(conversation_image_count(conversation));
    exchanges.extend(execute_chat_tool_batch(
        app,
        core,
        request_id,
        sequence,
        conversation,
        workspaces,
        operations,
        &tool_state.operation_ledger,
        &mut tool_state.path_references,
        &mut remaining_image_slots,
        cancellation_requested,
    ));
    Some(exchanges)
}

pub(in crate::chat_runtime::generation) fn current_reads_can_survive_rejected_plan(
    workspaces: &[crate::chat_tools::ChatToolWorkspace],
    plan: &mut PlanPolicy,
    operations: &[ParsedToolCall],
) -> bool {
    plan.active()
        && !operations.is_empty()
        && operations
            .iter()
            .all(|call| validated_chat_tool_request(call, workspaces).is_ok())
        && plan.validate_safe_current_reads(operations).is_ok()
}

#[allow(clippy::too_many_arguments)]
pub(in crate::chat_runtime::generation) fn handle_rejected_plan_admission(
    app: &tauri::AppHandle,
    core: &AppCore,
    request_id: &RequestId,
    sequence: &mut u32,
    conversation: &mut Conversation,
    tool_state: &mut GenerationToolState,
    telemetry: &mut RequestExecutionTelemetry,
    generated: ChatGenerationRound,
    round: usize,
    error: &IpcError,
) -> Option<ChatTerminal> {
    let call = &generated.calls[0];
    trace_tool_rejection(
        core,
        request_id,
        conversation,
        round,
        std::slice::from_ref(call),
        "orchestration_plan_admission",
        &error.message,
    );
    if !tool_state.plan_policy.correction_available(error) {
        return Some(terminal_error(
            tool_preparation_terminal_error(),
            GenerationFailureCategory::OrchestrationPhase,
        ));
    }
    let diagnostic = format!(
        "The plan declaration was rejected: {} Correct only the reported plan objects; prior useful workspace results remain retained.",
        error.message
    );
    let (recovery, progress) = if let Some(defects) = plan_repair_defects(error) {
        let recovery = tool_state.request_plan_recovery(
            tool_state.plan_policy.correction_scope(),
            defects,
            diagnostic,
        );
        (recovery, tool_state.plan_defect_progress())
    } else {
        (
            tool_state.request_recovery(
                RecoveryKind::PlanValidation,
                tool_state.plan_policy.correction_scope(),
                format!(
                    "{}:{:?}",
                    tool_call_fingerprint(std::slice::from_ref(call)),
                    error.code
                ),
                diagnostic,
            ),
            None,
        )
    };
    trace_plan_repair_decision(
        core,
        request_id,
        conversation,
        round,
        progress,
        recovery.is_ok(),
    );
    if let Err(terminal) = recovery {
        return Some(terminal);
    }
    tool_state.plan_policy.record_correction(error);
    let exchanges = reject_invalid_tool_batch(
        app,
        request_id,
        sequence,
        conversation,
        std::slice::from_ref(call),
        error,
    );
    telemetry.record_exchanges(&exchanges);
    trace_tool_exchanges(core, request_id, conversation, round, &exchanges);
    trace_plan_tool_results(
        core,
        request_id,
        conversation,
        round,
        &tool_state.plan_policy,
        std::slice::from_ref(call),
        &exchanges,
    );
    persist_unfinished_tool_batch(core, request_id, conversation, generated, exchanges)
}

#[allow(clippy::too_many_arguments)]
pub(in crate::chat_runtime::generation) fn reject_plan_prefixed_operation_batch(
    app: &tauri::AppHandle,
    request_id: &RequestId,
    sequence: &mut u32,
    conversation: &Conversation,
    operation_calls: &[ParsedToolCall],
    plan: &PlanPolicy,
    error: &IpcError,
) -> Vec<Result<ChatToolExchange, IpcError>> {
    if is_plan_relationship_error(error)
        || plan
            .pre_execution_evidence_deviation(operation_calls)
            .is_some()
    {
        reject_plan_deviation_batch(
            app,
            request_id,
            sequence,
            conversation,
            operation_calls,
            error,
        )
    } else {
        reject_invalid_tool_batch(
            app,
            request_id,
            sequence,
            conversation,
            operation_calls,
            error,
        )
    }
}

pub(in crate::chat_runtime::generation) fn is_plan_relationship_error(error: &IpcError) -> bool {
    matches!(
        error.code,
        IpcErrorCode::PlanMismatch | IpcErrorCode::ScopeCorrection
    )
}

#[allow(clippy::too_many_arguments)]
pub(in crate::chat_runtime::generation) fn finish_plan_prefixed_round(
    core: &AppCore,
    request_id: &RequestId,
    conversation: &mut Conversation,
    tool_state: &mut GenerationToolState,
    telemetry: &mut RequestExecutionTelemetry,
    generated: ChatGenerationRound,
    exchanges: Vec<Result<ChatToolExchange, IpcError>>,
    round: usize,
) -> Option<ChatTerminal> {
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
    let allow_automatic_completion = tool_state.plan_policy.allows_automatic_write_confirmation();
    persist_executed_tool_batch(
        core,
        request_id,
        conversation,
        generated,
        exchanges,
        allow_automatic_completion,
    )
}

#[allow(clippy::too_many_arguments)]
pub(in crate::chat_runtime::generation) fn correct_invalid_operation_after_accepted_plan(
    app: &tauri::AppHandle,
    request_id: &RequestId,
    sequence: &mut u32,
    conversation: &Conversation,
    workspaces: &[crate::chat_tools::ChatToolWorkspace],
    all_tools: &[PromptToolDefinition],
    tool_state: &mut GenerationToolState,
    calls: &[ParsedToolCall],
) -> Result<Option<Result<ChatToolExchange, IpcError>>, ChatTerminal> {
    let [_, operation] = calls else {
        return Ok(None);
    };
    if !plan_prefixed_invalid_operation_batch_repairable(calls, 1) {
        return Ok(None);
    }
    let Some(base_error) = validated_chat_tool_request(operation, workspaces).err() else {
        return Ok(None);
    };
    let error = exact_argument_repair_error(operation, all_tools, &base_error);
    tool_state.request_recovery(
        RecoveryKind::InvalidArguments,
        tool_state.plan_policy.correction_scope(),
        argument_correction_fingerprint(std::slice::from_ref(operation), &error),
        format!(
            "The plan was accepted, but its adjacent `{}` operation failed argument validation: {} Keep the accepted plan and repair only this operation.",
            operation.name, error.message
        ),
    )?;
    record_argument_correction(tool_state, std::slice::from_ref(operation), &error);
    Ok(reject_invalid_tool_batch(
        app,
        request_id,
        sequence,
        conversation,
        std::slice::from_ref(operation),
        &error,
    )
    .into_iter()
    .next())
}

pub(in crate::chat_runtime::generation) fn persist_executed_tool_batch(
    core: &AppCore,
    request_id: &RequestId,
    conversation: &mut Conversation,
    generated: ChatGenerationRound,
    exchanges: Vec<Result<ChatToolExchange, IpcError>>,
    allow_automatic_completion: bool,
) -> Option<ChatTerminal> {
    let applied_paths = applied_paths(&exchanges);
    let applied_changes_verified = applied_changes_verified(&exchanges);
    let batch_completed = tool_batch_completed(&exchanges);
    let file_content_mutation_completed = file_content_mutation_completed(&exchanges);
    let submitted_assets = generated.image_ids.clone();
    let binding_sha256 = generated.binding_sha256.clone();
    match persist_tool_exchanges(
        core,
        request_id,
        conversation,
        generated.reasoning,
        generated.content,
        exchanges,
        &submitted_assets,
        &binding_sha256,
    ) {
        Ok(())
            if allow_automatic_completion
                && file_content_mutation_completed
                && applied_changes_verified
                && batch_completed =>
        {
            Some(ChatTerminal::Final {
                reasoning: None,
                text: applied_change_confirmation(&applied_paths),
                reason: FinishReason::Stop,
                mutation_applied: true,
            })
        }
        Ok(()) if !applied_paths.is_empty() && !batch_completed => {
            Some(terminal_after_mutation(ChatTerminal::Cancelled, true))
        }
        Ok(()) => None,
        Err(error) => Some(terminal_after_mutation(
            terminal_error(error, GenerationFailureCategory::Persistence),
            !applied_paths.is_empty(),
        )),
    }
}

pub(in crate::chat_runtime::generation) fn persist_unfinished_tool_batch(
    core: &AppCore,
    request_id: &RequestId,
    conversation: &mut Conversation,
    generated: ChatGenerationRound,
    exchanges: Vec<Result<ChatToolExchange, IpcError>>,
) -> Option<ChatTerminal> {
    persist_executed_tool_batch(core, request_id, conversation, generated, exchanges, false)
}

pub(in crate::chat_runtime::generation) fn terminal_without_tool_calls(
    generated: &mut ChatGenerationRound,
    plan_policy: &PlanPolicy,
) -> Option<ChatTerminal> {
    if !generated.calls.is_empty() {
        return None;
    }
    if plan_policy.expects_file_mutation() && !plan_policy.planned_outcome_satisfied() {
        let clarification = generated.content.trim();
        if clarification.len() <= 1_000 && clarification.contains('?') {
            return Some(ChatTerminal::Final {
                reasoning: None,
                text: std::mem::take(&mut generated.content),
                reason: FinishReason::Stop,
                mutation_applied: false,
            });
        }
        return Some(terminal_error(
            IpcError {
                code: IpcErrorCode::Unsupported,
                message: "The response stopped before producing and verifying the file required by its provisional plan. Please retry the request.".to_owned(),
                retryable: true,
            },
            GenerationFailureCategory::OrchestrationPhase,
        ));
    }
    Some(ChatTerminal::Final {
        reasoning: (!generated.reasoning.is_empty())
            .then(|| std::mem::take(&mut generated.reasoning)),
        text: std::mem::take(&mut generated.content),
        reason: match generated.reason {
            GenerationCompletion::Stop => FinishReason::Stop,
            GenerationCompletion::Length => FinishReason::Length,
        },
        mutation_applied: plan_policy.planned_outcome_satisfied(),
    })
}

pub(in crate::chat_runtime::generation) const fn should_schedule_outcome_recovery(
    no_tool_calls: bool,
    typed_outcome_unmet: bool,
    round: usize,
) -> bool {
    no_tool_calls && typed_outcome_unmet && round < MAX_CHAT_TOOL_ROUNDS
}

pub(in crate::chat_runtime::generation) const fn should_schedule_forecast_recovery(
    no_tool_calls: bool,
    forecast_required: bool,
    round: usize,
) -> bool {
    no_tool_calls && forecast_required && round < MAX_CHAT_TOOL_ROUNDS
}
