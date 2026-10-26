use std::collections::BTreeMap;

use crate::{
    ExecutionLedger, LedgerNextAction, LedgerTerminalState, ObservationFact, PermissionBinding,
    ToolClassification, ToolOutcome, ToolResultEnvelope, ValidatedToolRequest,
};

use super::{
    AgentLoopSnapshot, FinalAnswerReason, GenerationUsage, LoopControlError, LoopDirective,
    LoopLimits, RoundAllowance, ToolCallIntent, ToolCallPlan, ToolExecutionUsage,
    types::{ActiveCall, EquivalentCall, FailedCall, LoopLimitsSnapshot},
};

const SNAPSHOT_VERSION: u16 = 1;
const MAX_TRACKED_CALLS: usize = 256;

/// Enforces bounded rounds, calls, retries, outputs and terminal recovery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentLoopController {
    limits: LoopLimits,
    consecutive_tool_rounds: u32,
    active: BTreeMap<String, ActiveCall>,
    successful: Vec<EquivalentCall>,
    failures: BTreeMap<String, FailedCall>,
    final_answer_reason: Option<FinalAnswerReason>,
    terminal: Option<LedgerTerminalState>,
}

impl AgentLoopController {
    /// Creates a controller with validated policy limits.
    #[must_use]
    pub fn new(limits: LoopLimits) -> Self {
        Self {
            limits,
            consecutive_tool_rounds: 0,
            active: BTreeMap::new(),
            successful: Vec::new(),
            failures: BTreeMap::new(),
            final_answer_reason: None,
            terminal: None,
        }
    }

    /// Reserves one bounded inference round and returns its remaining budgets.
    pub fn begin_round(&mut self, ledger: &mut ExecutionLedger) -> LoopDirective {
        if let Some(terminal) = self.terminal {
            return LoopDirective::Terminal(terminal);
        }
        if let Some(reason) = self.final_answer_reason {
            return LoopDirective::PrepareFinalAnswer(reason);
        }
        if ledger.has_authoritative_mutation() && !ledger.has_unresolved_failures() {
            return self.prepare_final(ledger, FinalAnswerReason::MutationApplied);
        }
        let mut budgets = ledger.budgets();
        if budgets.rounds == 0
            || budgets.tool_calls == 0
            || budgets.elapsed_ms == 0
            || budgets.context_tokens == 0
            || budgets.final_response_tokens == 0
            || self.consecutive_tool_rounds >= self.limits.consecutive_tool_rounds
        {
            return self.prepare_final(ledger, FinalAnswerReason::BudgetExhausted);
        }
        budgets.rounds -= 1;
        ledger.set_budgets(budgets);
        let _ = ledger.set_next_action(LedgerNextAction::Generate);
        LoopDirective::Generate(RoundAllowance {
            remaining: budgets,
            tools_enabled: true,
        })
    }

    /// Charges trusted generation usage and switches to recovery when exhausted.
    ///
    /// # Errors
    ///
    /// Returns an error for zero or otherwise malformed trusted usage.
    pub fn record_generation_usage(
        &mut self,
        ledger: &mut ExecutionLedger,
        usage: GenerationUsage,
    ) -> Result<(), LoopControlError> {
        if usage.generated_tokens == 0 || usage.elapsed_ms == 0 {
            return Err(LoopControlError::InvalidUsage);
        }
        let mut budgets = ledger.budgets();
        budgets.context_tokens = budgets
            .context_tokens
            .saturating_sub(usage.generated_tokens);
        budgets.elapsed_ms = budgets.elapsed_ms.saturating_sub(usage.elapsed_ms);
        ledger.set_budgets(budgets);
        if budgets.context_tokens == 0 || budgets.elapsed_ms == 0 {
            self.prepare_final(ledger, FinalAnswerReason::BudgetExhausted);
        }
        Ok(())
    }

    /// Validates, deduplicates and reserves one exact tool call.
    ///
    /// # Errors
    ///
    /// Returns an error when policy disables tools, rejects duplication, or
    /// cannot prove an exact bounded retry.
    pub fn plan_tool_call<'a>(
        &mut self,
        ledger: &mut ExecutionLedger,
        request: &ValidatedToolRequest,
        binding: &PermissionBinding,
        intent: &ToolCallIntent,
        reusable_observation: Option<&'a ObservationFact>,
    ) -> Result<ToolCallPlan<'a>, LoopControlError> {
        if self.terminal.is_some() {
            return Err(LoopControlError::Terminal);
        }
        if self.final_answer_reason.is_some()
            || (ledger.has_authoritative_mutation() && !ledger.has_unresolved_failures())
        {
            return Err(LoopControlError::ToolsDisabled);
        }
        if !binding.matches_request_only(request)
            || binding.resource_scope().project_id() != ledger.request().project_id()
        {
            return Err(LoopControlError::RetryScopeMismatch);
        }
        let key = EquivalentCall::new(request, binding);
        if self.successful.iter().any(|item| item.equivalent(&key)) {
            if request.definition().classification() == ToolClassification::Read
                && let Some(observation) = reusable_observation
            {
                return Ok(ToolCallPlan::ReuseObservation(observation));
            }
            return Err(LoopControlError::DuplicateCall);
        }
        match intent {
            ToolCallIntent::New => {
                if self
                    .failures
                    .values()
                    .any(|failed| failed.key.equivalent(&key))
                {
                    return Err(LoopControlError::RetryRequired);
                }
            }
            ToolCallIntent::Retry { failed_call_id } => {
                let failed = self
                    .failures
                    .get(failed_call_id)
                    .ok_or(LoopControlError::RetryScopeMismatch)?;
                if !failed.key.equivalent(&key) {
                    return Err(LoopControlError::RetryScopeMismatch);
                }
                if !failed.transient {
                    return Err(LoopControlError::NonTransientFailure);
                }
                if failed.count >= self.limits.repeated_failures {
                    self.prepare_final(ledger, FinalAnswerReason::RepeatedFailure);
                    return Err(LoopControlError::ToolsDisabled);
                }
            }
        }
        let mut budgets = ledger.budgets();
        if budgets.tool_calls == 0 || budgets.tool_ms == 0 || budgets.tool_output_bytes == 0 {
            self.prepare_final(ledger, FinalAnswerReason::BudgetExhausted);
            return Err(LoopControlError::ToolsDisabled);
        }
        if self.active.contains_key(request.call_id())
            || self.active.len() + self.successful.len() + self.failures.len() >= MAX_TRACKED_CALLS
        {
            return Err(LoopControlError::InvalidUsage);
        }
        budgets.tool_calls -= 1;
        ledger.set_budgets(budgets);
        self.active.insert(
            request.call_id().to_owned(),
            ActiveCall {
                key,
                classification: request.definition().classification(),
            },
        );
        let _ = ledger.set_next_action(LedgerNextAction::ExecuteTool {
            call_id: request.call_id().to_owned(),
        });
        Ok(ToolCallPlan::Execute)
    }

    /// Charges one exact terminal tool result and records retry policy state.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed usage or a result not bound to the active call.
    pub fn record_tool_result(
        &mut self,
        ledger: &mut ExecutionLedger,
        result: &ToolResultEnvelope,
        usage: ToolExecutionUsage,
    ) -> Result<(), LoopControlError> {
        if usage.elapsed_ms == 0 {
            return Err(LoopControlError::InvalidUsage);
        }
        let active = self
            .active
            .get(result.call_id())
            .ok_or(LoopControlError::ResultMismatch)?;
        if result.tool_name() != active.key.tool_name {
            return Err(LoopControlError::ResultMismatch);
        }
        let active = self
            .active
            .remove(result.call_id())
            .ok_or(LoopControlError::ResultMismatch)?;
        let output_bytes = u64::try_from(
            serde_json::to_vec(result)
                .map_err(|_| LoopControlError::InvalidUsage)?
                .len(),
        )
        .map_err(|_| LoopControlError::InvalidUsage)?;
        let mut budgets = ledger.budgets();
        budgets.tool_ms = budgets.tool_ms.saturating_sub(usage.elapsed_ms);
        budgets.elapsed_ms = budgets.elapsed_ms.saturating_sub(usage.elapsed_ms);
        budgets.tool_output_bytes = budgets.tool_output_bytes.saturating_sub(output_bytes);
        ledger.set_budgets(budgets);
        self.consecutive_tool_rounds = self.consecutive_tool_rounds.saturating_add(1);
        match result.outcome() {
            ToolOutcome::Completed { .. } => self.successful.push(active.key),
            ToolOutcome::Failed { failure } => {
                let prior_count = u32::try_from(
                    self.failures
                        .values()
                        .filter(|prior| prior.key.equivalent(&active.key))
                        .count(),
                )
                .map_err(|_| LoopControlError::InvalidUsage)?;
                let count = prior_count
                    .checked_add(1)
                    .ok_or(LoopControlError::InvalidUsage)?;
                self.failures.insert(
                    result.call_id().to_owned(),
                    FailedCall {
                        key: active.key,
                        transient: failure.is_transient(),
                        count,
                    },
                );
                if !failure.is_transient() || count >= self.limits.repeated_failures {
                    self.prepare_final(ledger, FinalAnswerReason::RepeatedFailure);
                }
            }
            ToolOutcome::Cancelled { .. } => {}
        }
        if budgets.tool_ms == 0 || budgets.elapsed_ms == 0 || budgets.tool_output_bytes == 0 {
            self.prepare_final(ledger, FinalAnswerReason::BudgetExhausted);
        }
        Ok(())
    }

    /// Charges the bounded final-response-only generation reserve.
    ///
    /// Returns `true` only when the response fit completely inside the reserve.
    ///
    /// # Errors
    ///
    /// Returns an error for zero or otherwise malformed trusted usage.
    pub fn record_final_response_usage(
        &mut self,
        ledger: &mut ExecutionLedger,
        generated_tokens: u64,
        elapsed_ms: u64,
    ) -> Result<bool, LoopControlError> {
        if generated_tokens == 0 || elapsed_ms == 0 {
            return Err(LoopControlError::InvalidUsage);
        }
        let mut budgets = ledger.budgets();
        let complete =
            generated_tokens <= budgets.final_response_tokens && elapsed_ms <= budgets.elapsed_ms;
        budgets.final_response_tokens = budgets
            .final_response_tokens
            .saturating_sub(generated_tokens);
        budgets.elapsed_ms = budgets.elapsed_ms.saturating_sub(elapsed_ms);
        ledger.set_budgets(budgets);
        Ok(complete)
    }

    /// Resets the consecutive-tool counter after a final answer did not execute a tool.
    pub const fn note_non_tool_round(&mut self) {
        self.consecutive_tool_rounds = 0;
    }

    /// Marks cancellation terminal before the host stops outstanding work.
    pub fn cancel(&mut self, ledger: &mut ExecutionLedger) {
        self.terminal = Some(LedgerTerminalState::Cancelled);
        // Once cancellation is authoritative, late results from formerly active
        // calls must not be chargeable or promotable into trusted facts.
        self.active.clear();
        let _ = ledger.set_next_action(LedgerNextAction::Terminal {
            state: LedgerTerminalState::Cancelled,
        });
    }

    /// Derives and records the exact terminal outcome.
    pub fn finish(
        &mut self,
        ledger: &mut ExecutionLedger,
        task_succeeded: bool,
        response_completed: bool,
    ) -> LedgerTerminalState {
        let state = if self.terminal == Some(LedgerTerminalState::Cancelled) {
            LedgerTerminalState::Cancelled
        } else if ledger.has_authoritative_mutation() && !response_completed {
            LedgerTerminalState::ChangeAppliedResponseIncomplete
        } else if ledger.has_partial_outcomes()
            || ledger.has_unresolved_failures()
            || (!task_succeeded && ledger.has_authoritative_mutation())
        {
            LedgerTerminalState::PartialOutcome
        } else if task_succeeded && response_completed {
            LedgerTerminalState::CompleteSuccess
        } else {
            LedgerTerminalState::NoChangeApplied
        };
        self.terminal = Some(state);
        self.final_answer_reason = None;
        let _ = ledger.set_next_action(LedgerNextAction::Terminal { state });
        state
    }

    /// Exports bounded restart state.
    #[must_use]
    pub fn snapshot(&self) -> AgentLoopSnapshot {
        AgentLoopSnapshot {
            version: SNAPSHOT_VERSION,
            limits: LoopLimitsSnapshot {
                consecutive_tool_rounds: self.limits.consecutive_tool_rounds,
                repeated_failures: self.limits.repeated_failures,
            },
            consecutive_tool_rounds: self.consecutive_tool_rounds,
            active: self.active.clone(),
            successful: self.successful.clone(),
            failures: self.failures.clone(),
            final_answer_reason: self.final_answer_reason,
            terminal: self.terminal,
        }
    }

    /// Restores validated bounded loop policy state.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed, inconsistent, or oversized restart state.
    pub fn restore(snapshot: AgentLoopSnapshot) -> Result<Self, LoopControlError> {
        let limits = LoopLimits::new(
            snapshot.limits.consecutive_tool_rounds,
            snapshot.limits.repeated_failures,
        )?;
        if snapshot.version != SNAPSHOT_VERSION
            || snapshot.active.len() + snapshot.successful.len() + snapshot.failures.len()
                > MAX_TRACKED_CALLS
            || snapshot.consecutive_tool_rounds > limits.consecutive_tool_rounds
        {
            return Err(LoopControlError::InvalidUsage);
        }
        if snapshot.final_answer_reason.is_some() && snapshot.terminal.is_some() {
            return Err(LoopControlError::InvalidUsage);
        }
        for (call_id, active) in &snapshot.active {
            validate_call_key(&active.key)?;
            if call_id != active.key.binding.call_id() {
                return Err(LoopControlError::InvalidUsage);
            }
        }
        for successful in &snapshot.successful {
            validate_call_key(successful)?;
        }
        for (call_id, failed) in &snapshot.failures {
            validate_call_key(&failed.key)?;
            if call_id != failed.key.binding.call_id()
                || failed.count == 0
                || failed.count > limits.repeated_failures
            {
                return Err(LoopControlError::InvalidUsage);
            }
        }
        Ok(Self {
            limits,
            consecutive_tool_rounds: snapshot.consecutive_tool_rounds,
            active: snapshot.active,
            successful: snapshot.successful,
            failures: snapshot.failures,
            final_answer_reason: snapshot.final_answer_reason,
            terminal: snapshot.terminal,
        })
    }

    fn prepare_final(
        &mut self,
        ledger: &mut ExecutionLedger,
        reason: FinalAnswerReason,
    ) -> LoopDirective {
        self.final_answer_reason = Some(reason);
        let _ = ledger.set_next_action(LedgerNextAction::PrepareFinalAnswer);
        LoopDirective::PrepareFinalAnswer(reason)
    }
}

fn validate_call_key(key: &EquivalentCall) -> Result<(), LoopControlError> {
    key.binding
        .validate()
        .map_err(|_| LoopControlError::InvalidUsage)?;
    if key.tool_name != key.binding.tool_name()
        || key.arguments_sha256 != key.binding.arguments_sha256()
        || key.capability != key.binding.capability()
    {
        return Err(LoopControlError::InvalidUsage);
    }
    Ok(())
}
