#[cfg(debug_assertions)]
use super::*;
#[cfg(not(debug_assertions))]
use super::{
    AppCore, ChatGenerationRound, ChatTerminal, ChatToolExchange, Conversation, Instant, IpcError,
    RequestId,
};

#[cfg(debug_assertions)]
#[derive(Clone, Copy, Default)]
pub(in crate::chat_runtime::generation) struct RoundTiming {
    pub(in crate::chat_runtime::generation) preparation: u64,
    pub(in crate::chat_runtime::generation) generation_call: u64,
    pub(in crate::chat_runtime::generation) output_token_measurement: u64,
    pub(in crate::chat_runtime::generation) provider_total: Option<u64>,
}

pub(in crate::chat_runtime::generation) struct RequestExecutionTelemetry {
    #[cfg(debug_assertions)]
    pub(in crate::chat_runtime::generation) started: Instant,
    #[cfg(debug_assertions)]
    pub(in crate::chat_runtime::generation) rounds: usize,
    #[cfg(debug_assertions)]
    attempted_rounds: usize,
    #[cfg(debug_assertions)]
    failed_rounds: usize,
    #[cfg(debug_assertions)]
    pub(in crate::chat_runtime::generation) calls: usize,
    #[cfg(debug_assertions)]
    pub(in crate::chat_runtime::generation) plan_revisions: usize,
    #[cfg(debug_assertions)]
    pub(in crate::chat_runtime::generation) submitted_images: usize,
    #[cfg(debug_assertions)]
    pub(in crate::chat_runtime::generation) repeated_rich_inputs: usize,
    #[cfg(debug_assertions)]
    image_counts: crate::chat_runtime::rich_input::RichInputCounts,
    #[cfg(debug_assertions)]
    pub(in crate::chat_runtime::generation) first_action_ms: Option<u64>,
    #[cfg(debug_assertions)]
    first_task_result_ms: Option<u64>,
    #[cfg(debug_assertions)]
    pub(in crate::chat_runtime::generation) first_mutation_ms: Option<u64>,
    #[cfg(debug_assertions)]
    pub(in crate::chat_runtime::generation) verified_outcome_ms: Option<u64>,
    #[cfg(debug_assertions)]
    pub(in crate::chat_runtime::generation) accounted_ms: u64,
    #[cfg(debug_assertions)]
    pub(in crate::chat_runtime::generation) provider_reported_ms: u64,
    #[cfg(debug_assertions)]
    pub(in crate::chat_runtime::generation) recovery_ms: u64,
}

impl RequestExecutionTelemetry {
    pub(in crate::chat_runtime::generation) fn new() -> Self {
        Self {
            #[cfg(debug_assertions)]
            started: Instant::now(),
            #[cfg(debug_assertions)]
            rounds: 0,
            #[cfg(debug_assertions)]
            attempted_rounds: 0,
            #[cfg(debug_assertions)]
            failed_rounds: 0,
            #[cfg(debug_assertions)]
            calls: 0,
            #[cfg(debug_assertions)]
            plan_revisions: 0,
            #[cfg(debug_assertions)]
            submitted_images: 0,
            #[cfg(debug_assertions)]
            repeated_rich_inputs: 0,
            #[cfg(debug_assertions)]
            image_counts: crate::chat_runtime::rich_input::RichInputCounts::default(),
            #[cfg(debug_assertions)]
            first_action_ms: None,
            #[cfg(debug_assertions)]
            first_task_result_ms: None,
            #[cfg(debug_assertions)]
            first_mutation_ms: None,
            #[cfg(debug_assertions)]
            verified_outcome_ms: None,
            #[cfg(debug_assertions)]
            accounted_ms: 0,
            #[cfg(debug_assertions)]
            provider_reported_ms: 0,
            #[cfg(debug_assertions)]
            recovery_ms: 0,
        }
    }

    pub(in crate::chat_runtime::generation) fn record_round(
        &mut self,
        round: &ChatGenerationRound,
    ) {
        #[cfg(debug_assertions)]
        {
            self.rounds = self.rounds.saturating_add(1);
            self.attempted_rounds = self.attempted_rounds.saturating_add(1);
            self.calls = self.calls.saturating_add(round.calls.len());
            if !round.calls.is_empty() && self.first_action_ms.is_none() {
                self.first_action_ms = Some(elapsed_ms(self.started));
            }
            self.accounted_ms = self
                .accounted_ms
                .saturating_add(round.timing.preparation)
                .saturating_add(round.timing.generation_call)
                .saturating_add(round.timing.output_token_measurement);
            self.provider_reported_ms = self
                .provider_reported_ms
                .saturating_add(round.timing.provider_total.unwrap_or(0));
        }
        #[cfg(not(debug_assertions))]
        let _ = (self, round);
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::chat_runtime::generation) fn record_failed_round(
        &mut self,
        core: &AppCore,
        request_id: &RequestId,
        conversation: &Conversation,
        round: usize,
        duration_ms: u64,
        terminal: &ChatTerminal,
    ) {
        #[cfg(debug_assertions)]
        {
            self.accounted_ms = self.accounted_ms.saturating_add(duration_ms);
            self.attempted_rounds = self.attempted_rounds.saturating_add(1);
            let failed = matches!(terminal, ChatTerminal::Error { .. });
            self.failed_rounds = self.failed_rounds.saturating_add(usize::from(failed));
            core.record_development_tool_trace(&serde_json::json!({
                "event": "execution_round_attempt_finished",
                "requestId": request_id.0,
                "conversationId": conversation.id.0,
                "round": round,
                "durationMs": duration_ms,
                "outcome": if failed { "failed" } else { "cancelled" }
            }));
        }
        #[cfg(not(debug_assertions))]
        let _ = (
            self,
            core,
            request_id,
            conversation,
            round,
            duration_ms,
            terminal,
        );
    }

    pub(in crate::chat_runtime::generation) fn record_image_counts(
        &mut self,
        counts: crate::chat_runtime::rich_input::RichInputCounts,
    ) {
        #[cfg(debug_assertions)]
        {
            self.image_counts = counts;
            self.submitted_images = counts.presentations;
            self.repeated_rich_inputs = counts.presentations.saturating_sub(counts.unique_sources);
        }
        #[cfg(not(debug_assertions))]
        let _ = (self, counts);
    }

    pub(in crate::chat_runtime::generation) fn set_plan_revision(&mut self, revision: usize) {
        #[cfg(debug_assertions)]
        {
            self.plan_revisions = revision;
        }
        #[cfg(not(debug_assertions))]
        let _ = (self, revision);
    }

    pub(in crate::chat_runtime::generation) fn record_exchanges(
        &mut self,
        exchanges: &[Result<ChatToolExchange, IpcError>],
    ) {
        #[cfg(debug_assertions)]
        {
            if self.first_task_result_ms.is_none()
                && exchanges
                    .iter()
                    .filter_map(|e| e.as_ref().ok())
                    .any(|e| completed_task_result(&e.request.name, &e.result.outcome))
            {
                self.first_task_result_ms = Some(elapsed_ms(self.started));
            }
            let file_content_mutated = file_content_mutation_completed(exchanges);
            if self.first_mutation_ms.is_none() && file_content_mutated {
                self.first_mutation_ms = Some(elapsed_ms(self.started));
            }
            if self.verified_outcome_ms.is_none()
                && file_content_mutated
                && applied_changes_verified(exchanges)
            {
                self.verified_outcome_ms = Some(elapsed_ms(self.started));
            }
        }
        #[cfg(not(debug_assertions))]
        let _ = (self, exchanges);
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::chat_runtime::generation) fn finish_round_post(
        &mut self,
        core: &AppCore,
        request_id: &RequestId,
        conversation: &Conversation,
        round: usize,
        started: Instant,
        tool_work: bool,
        recovery: bool,
    ) {
        #[cfg(debug_assertions)]
        {
            let duration_ms = elapsed_ms(started);
            self.accounted_ms = self.accounted_ms.saturating_add(duration_ms);
            if recovery {
                self.recovery_ms = self.recovery_ms.saturating_add(duration_ms);
            }
            core.record_development_tool_trace(&serde_json::json!({
                "event": "execution_round_host_completed",
                "requestId": request_id.0,
                "conversationId": conversation.id.0,
                "round": round,
                "toolExecutionMs": tool_work.then_some(duration_ms),
                "hostOrchestrationMs": (!tool_work).then_some(duration_ms),
                "recovery": recovery
            }));
        }
        #[cfg(not(debug_assertions))]
        let _ = (
            self,
            core,
            request_id,
            conversation,
            round,
            started,
            tool_work,
            recovery,
        );
    }

    pub(in crate::chat_runtime::generation) fn finish(
        &self,
        core: &AppCore,
        request_id: &RequestId,
        conversation: &Conversation,
        terminal: &ChatTerminal,
    ) {
        #[cfg(debug_assertions)]
        {
            let total_ms = elapsed_ms(self.started);
            let verified = matches!(terminal, ChatTerminal::Final { .. });
            core.record_development_tool_trace(&serde_json::json!({
                "event": "execution_request_completed",
                "requestId": request_id.0,
                "conversationId": conversation.id.0,
                "totalMs": total_ms,
                "timeToFirstActionMs": self.first_action_ms,
                "timeToFirstCompletedTaskResultMs": self.first_task_result_ms,
                "timeToFirstMutationMs": self.first_mutation_ms,
                "timeToVerifiedOutcomeMs": self.verified_outcome_ms.or_else(|| verified.then_some(total_ms)),
                "totalRounds": self.rounds,
                "attemptedRounds": self.attempted_rounds,
                "failedRounds": self.failed_rounds,
                "totalCalls": self.calls,
                "submittedImages": self.submitted_images,
                "repeatedRichInputs": self.repeated_rich_inputs,
                "uniqueImageSources": self.image_counts.unique_sources,
                "explicitImageInspections": self.image_counts.explicit_inspections,
                "retainedImagePresentations": self.image_counts.retained_presentations,
                "planRevisions": self.plan_revisions,
                "recoveryMs": self.recovery_ms,
                "providerReportedMs": self.provider_reported_ms,
                "idleOrUnclassifiedMs": total_ms.saturating_sub(self.accounted_ms),
                "outcome": match terminal {
                    ChatTerminal::Final { .. } => "completed",
                    ChatTerminal::Cancelled => "cancelled",
                    ChatTerminal::Error { .. } => "failed",
                }
            }));
        }
        #[cfg(not(debug_assertions))]
        let _ = (self, core, request_id, conversation, terminal);
    }
}

pub(in crate::chat_runtime::generation) fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(debug_assertions)]
fn completed_task_result(name: &ipc_types::ToolName, outcome: &ToolOutcome) -> bool {
    use ipc_types::ToolName;
    matches!(outcome, ToolOutcome::Completed { .. })
        && !matches!(
            name,
            ToolName::OrchestrationSubmitPlan
                | ToolName::OrchestrationSubmitForecast
                | ToolName::OrchestrationCompleteStage
                | ToolName::OrchestrationSearchTools
                | ToolName::OrchestrationRecall
                | ToolName::OrchestrationRefineStage
                | ToolName::OrchestrationToolCorrection
        )
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::*;
    #[test]
    fn task_result_timing_excludes_planning_and_deferred_calls() {
        use ipc_types::ToolName;
        let completed = ToolOutcome::Completed {
            content: Vec::new(),
        };
        assert!(!completed_task_result(
            &ToolName::OrchestrationSubmitPlan,
            &completed
        ));
        assert!(!completed_task_result(
            &ToolName::OrchestrationRecall,
            &completed
        ));
        assert!(completed_task_result(&ToolName::X402Request, &completed));
        assert!(completed_task_result(
            &ToolName::FilesystemReadText,
            &completed
        ));
        let deferred = ToolOutcome::Failed {
            error: IpcError::invalid_request("plan_pending"),
        };
        assert!(!completed_task_result(&ToolName::X402Request, &deferred));
    }
}
