//! Task-call admission; advisory checkpoints remain bounded by inference rounds.

use super::dispatch::stage_completion_boundary;
use super::{GenerationToolState, MAX_CHAT_TOOL_CALLS, MAX_CHAT_TOOL_ROUNDS, ParsedToolCall};

impl GenerationToolState {
    pub(super) fn budgeted_call_count(&self, calls: &[ParsedToolCall]) -> usize {
        // Exempt only one structurally valid, argument-free v6 checkpoint.
        // This is accounting, not stage admission or permission to execute siblings.
        let checkpoint = self.plan_policy.uses_advisory_stages()
            && stage_completion_boundary(calls).is_ok_and(|(_, call)| call.arguments.is_empty());
        calls.len().saturating_sub(usize::from(checkpoint))
    }

    pub(super) fn admits_tool_batch(&self, round: usize, calls: &[ParsedToolCall]) -> bool {
        tool_batch_within_limits(round, self.total_calls, self.budgeted_call_count(calls))
    }
}

pub(super) fn tool_batch_within_limits(
    round: usize,
    completed_calls: usize,
    batch_calls: usize,
) -> bool {
    round < MAX_CHAT_TOOL_ROUNDS
        && completed_calls
            .checked_add(batch_calls)
            .is_some_and(|total| total <= MAX_CHAT_TOOL_CALLS)
}
