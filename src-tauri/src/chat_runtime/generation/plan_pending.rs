//! Request-local planning intent, never an executable queue or repair allowance.

use super::*;
use crate::chat_tools::{
    PLAN_PENDING_GUIDANCE, PLAN_REQUIRED_OPERATION, deferred_plan_exchanges, emit_plan_deferrals,
    plan_deferral_batch_is_bounded,
};

#[derive(Clone, Debug, Eq, PartialEq)]
struct PendingOperation {
    operation: String,
    original_call_id: String,
    argument_sha256: String,
}

#[derive(Debug)]
pub(super) struct PlanPending {
    operations: Vec<PendingOperation>,
    unchanged_retries: usize,
}

impl PlanPending {
    #[cfg(any(debug_assertions, test))]
    fn trace_projection(&self) -> serde_json::Value {
        serde_json::json!({
            "requiredNextOperation": PLAN_REQUIRED_OPERATION,
            "unchangedRetries": self.unchanged_retries,
            "operations": self.operations.iter().map(|operation| serde_json::json!({
                "operation": operation.operation,
                "originalCallId": operation.original_call_id,
                "argumentSha256": operation.argument_sha256,
            })).collect::<Vec<_>>(),
        })
    }
    fn from_calls(calls: &[ParsedToolCall]) -> Self {
        Self {
            operations: calls
                .iter()
                .map(|call| PendingOperation {
                    operation: call.name.clone(),
                    original_call_id: call.call_id.clone(),
                    // BTreeMap arguments and recursively ordered serde_json maps give
                    // one canonical identity, independent of model call IDs.
                    argument_sha256: format!(
                        "{:x}",
                        Sha256::digest(serde_json::json!(call.arguments).to_string().as_bytes())
                    ),
                })
                .collect(),
            unchanged_retries: 0,
        }
    }

    pub(super) fn diagnostic(&self) -> app_core::ToolRecoveryDiagnostic {
        app_core::ToolRecoveryDiagnostic::new(
            "plan_pending".to_owned(),
            format!("requiredNextOperation={PLAN_REQUIRED_OPERATION}. {PLAN_PENDING_GUIDANCE}"),
            self.unchanged_retries + 1,
            2,
        )
    }
}

impl GenerationToolState {
    pub(super) fn pending_recovery_diagnostic(&self) -> Option<app_core::ToolRecoveryDiagnostic> {
        self.plan_pending
            .as_ref()
            .map(PlanPending::diagnostic)
            .or_else(|| self.recovery.pending())
    }

    fn register_pending(
        &mut self,
        calls: &[ParsedToolCall],
        round: usize,
    ) -> Result<(), ChatTerminal> {
        let incoming = PlanPending::from_calls(calls);
        let same = self.plan_pending.as_ref().is_some_and(|pending| {
            pending.operations.len() == incoming.operations.len()
                && pending
                    .operations
                    .iter()
                    .zip(&incoming.operations)
                    .all(|(a, b)| {
                        a.operation == b.operation && a.argument_sha256 == b.argument_sha256
                    })
        });
        if same {
            if let Some(pending) = &mut self.plan_pending {
                pending.unchanged_retries = pending.unchanged_retries.saturating_add(1);
            }
        } else {
            self.plan_pending = Some(incoming);
        }
        self.record_calls(calls);
        // Context references only: never record deferred calls in the operation
        // ledger or as useful results. Acceptance retains these paired failures.
        self.preplan_result_call_ids
            .extend(calls.iter().map(|call| call.call_id.clone()));
        if round >= MAX_CHAT_TOOL_ROUNDS {
            return Err(pending_stop(
                PendingStop::RoundLimit,
                GenerationFailureCategory::ToolLimit,
            ));
        }
        if self.total_calls >= MAX_CHAT_TOOL_CALLS {
            return Err(pending_stop(
                PendingStop::CallLimit,
                GenerationFailureCategory::ToolLimit,
            ));
        }
        if self
            .plan_pending
            .as_ref()
            .is_some_and(|pending| pending.unchanged_retries > 1)
        {
            return Err(pending_stop(
                PendingStop::UnchangedRepetition,
                GenerationFailureCategory::OrchestrationPhase,
            ));
        }
        Ok(())
    }
}

// This boundary binds separately owned request state, persistence, UI events and
// cancellation. Keeping those lifetimes explicit is clearer than a new container.
#[allow(clippy::too_many_arguments)]
pub(super) fn defer_preplan_calls(
    app: &tauri::AppHandle,
    core: &AppCore,
    request_id: &RequestId,
    sequence: &mut u32,
    conversation: &mut Conversation,
    state: &mut GenerationToolState,
    generated: &mut ChatGenerationRound,
    round: usize,
    cancellation: &AtomicBool,
) -> Result<bool, ChatTerminal> {
    if cancellation.load(Ordering::Acquire) {
        return Err(ChatTerminal::Cancelled);
    }
    if !requires_deferral(state, &generated.calls) {
        return Ok(false);
    }
    let exchanges = deferred_plan_exchanges(&generated.calls)
        .map_err(|error| terminal_error(error, GenerationFailureCategory::ToolValidation))?;
    // Even the terminal repetition/limit batch gets paired persisted results.
    let decision = state.register_pending(&generated.calls, round);
    // Do not use generic exchange tracing here: deferred original arguments
    // belong only in conversation history, never in development diagnostics.
    #[cfg(debug_assertions)]
    core.record_development_tool_trace(&serde_json::json!({
        "event": "plan_pending",
        "requestId": request_id.0,
        "conversationId": conversation.id.0,
        "round": round,
        "resume": decision.is_ok(),
        "checkpoint": state.plan_pending.as_ref().map(PlanPending::trace_projection),
    }));
    emit_plan_deferrals(app, request_id, sequence, conversation, &exchanges);
    recovery_flow::persist_recovery_exchange(
        core,
        request_id,
        conversation,
        generated,
        exchanges.into_iter().map(Ok).collect(),
    )?;
    if cancellation.load(Ordering::Acquire) {
        return Err(ChatTerminal::Cancelled);
    }
    decision?;
    if generated.reason != GenerationCompletion::Stop {
        return Err(pending_stop(
            PendingStop::OutputLimit,
            GenerationFailureCategory::ToolLimit,
        ));
    }
    Ok(true)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingStop {
    UnchangedRepetition,
    CallLimit,
    RoundLimit,
    OutputLimit,
    ModelStopped,
}

impl PendingStop {
    const fn as_str(self) -> &'static str {
        match self {
            Self::UnchangedRepetition => "unchanged_repetition",
            Self::CallLimit => "call_limit",
            Self::RoundLimit => "round_limit",
            Self::OutputLimit => "output_limit",
            Self::ModelStopped => "model_stopped",
        }
    }
}

fn requires_deferral(state: &GenerationToolState, calls: &[ParsedToolCall]) -> bool {
    !state.plan_policy.active()
        && !recovery_flow::is_direct_discovery_call_batch(calls)
        && plan_deferral_batch_is_bounded(calls)
}

fn pending_stop(cause: PendingStop, category: GenerationFailureCategory) -> ChatTerminal {
    let cause = cause.as_str();
    terminal_error(
        IpcError {
            code: IpcErrorCode::Unsupported,
            message: format!(
                "plan_not_submitted: {cause}. The deferred task calls were not executed. The required next operation remains {PLAN_REQUIRED_OPERATION}, submitted alone. Earlier completed work is retained."
            ),
            retryable: true,
        },
        category,
    )
}

pub(super) fn tool_limit_terminal(
    state: &GenerationToolState,
    round: usize,
    calls: &[ParsedToolCall],
) -> ChatTerminal {
    if state.plan_pending.is_none() {
        return terminal_error(
            tool_loop_limit_error(),
            GenerationFailureCategory::ToolLimit,
        );
    }
    pending_stop(
        if round >= MAX_CHAT_TOOL_ROUNDS {
            PendingStop::RoundLimit
        } else if state.total_calls.saturating_add(calls.len()) > MAX_CHAT_TOOL_CALLS {
            PendingStop::CallLimit
        } else {
            PendingStop::OutputLimit
        },
        GenerationFailureCategory::ToolLimit,
    )
}

pub(super) fn annotate_terminal(
    terminal: ChatTerminal,
    pending: Option<&PlanPending>,
) -> ChatTerminal {
    if pending.is_none() {
        return terminal;
    }
    match terminal {
        ChatTerminal::Cancelled => ChatTerminal::Cancelled,
        ChatTerminal::Final { reason, .. } => pending_stop(
            if reason == FinishReason::Length {
                PendingStop::OutputLimit
            } else {
                PendingStop::ModelStopped
            },
            GenerationFailureCategory::OrchestrationPhase,
        ),
        ChatTerminal::Error {
            mut error,
            category,
        } => {
            if !error.message.starts_with("plan_not_submitted:") {
                error.message = format!(
                    "plan_pending: requiredNextOperation={PLAN_REQUIRED_OPERATION} (submit alone). Stopped before plan acceptance: {}",
                    error.message
                );
            }
            ChatTerminal::Error { error, category }
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod acceptance_tests;
