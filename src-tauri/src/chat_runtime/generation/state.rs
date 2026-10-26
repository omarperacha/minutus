//! Per-request generation state and stable provider-prefix tracking.

use super::*;

#[cfg(test)]
mod tests;

#[derive(Default)]
pub(super) struct GenerationToolState {
    // Request-budget charges, excluding valid v6 advisory checkpoints. Telemetry
    // independently records every emitted call, including checkpoints/rejections.
    pub(super) total_calls: usize,
    pub(super) recall_scope: super::recall_scope::RecallScope,
    pub(super) consecutive_discovery_calls: usize,
    pub(super) phase_correction_scopes: BTreeMap<String, usize>,
    pub(super) argument_corrections: usize,
    pub(super) registry_corrections: usize,
    pub(super) plan_policy: PlanPolicy,
    pub(super) operation_ledger: OperationLedger,
    pub(super) path_references: PathReferenceLedger,
    pub(super) rich_input_presentations: RichInputPresentationLedger,
    pub(super) stable_execution_prefix: Option<StableExecutionPrefix>,
    pub(super) prompt_continuation: app_core::ChatPromptContinuation,
    pub(super) outcome_recovery_used: bool,
    pub(super) outcome_recovery_attempts: usize,
    pub(super) outcome_recovery_from_length: bool,
    pub(super) post_write_decision_pending: bool,
    pub(super) active_stage_contract: Option<BoundedStageContract>,
    pub(super) bounded_stage_failures: BTreeMap<String, usize>,
    pub(super) recovery: RecoveryCoordinator,
    pub(super) plan_pending: Option<super::plan_pending::PlanPending>,
    pub(super) preplan_result_call_ids: Vec<String>,
}

pub(super) struct StableExecutionPrefix {
    pub(super) boundary: ProviderPromptPrefixBoundary,
    pub(super) sha256: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ProviderPromptPrefixBoundary {
    pub(super) plan_revision: usize,
    pub(super) tool_surface_revision: usize,
    pub(super) x402_attempt_consumed: bool,
    pub(super) recall_available: bool,
}

#[derive(Default)]
pub(super) struct ProviderPromptPrefixTracker {
    #[cfg(any(debug_assertions, test))]
    pub(super) previous: Option<(ProviderPromptPrefixBoundary, String)>,
}

#[cfg(any(debug_assertions, test))]
impl ProviderPromptPrefixTracker {
    pub(super) fn observe(
        &mut self,
        boundary: ProviderPromptPrefixBoundary,
        rendered: &str,
    ) -> Option<String> {
        let eligible = self
            .previous
            .as_ref()
            .filter(|(previous_boundary, _)| *previous_boundary == boundary)
            .and_then(|(_, previous)| longest_common_rendered_prefix(previous, rendered));
        self.previous = Some((boundary, rendered.to_owned()));
        eligible
    }
}

#[cfg(any(debug_assertions, test))]
pub(super) fn longest_common_rendered_prefix(previous: &str, current: &str) -> Option<String> {
    let mut bytes = previous
        .as_bytes()
        .iter()
        .zip(current.as_bytes())
        .take_while(|(left, right)| left == right)
        .count();
    while bytes > 0 && !previous.is_char_boundary(bytes) {
        bytes -= 1;
    }
    (bytes > 0).then(|| previous[..bytes].to_owned())
}

impl GenerationToolState {
    pub(super) fn provider_prefix_boundary(&self) -> ProviderPromptPrefixBoundary {
        ProviderPromptPrefixBoundary {
            plan_revision: self.plan_policy.revision(),
            tool_surface_revision: self.plan_policy.tool_surface_revision(),
            x402_attempt_consumed: self.operation_ledger.x402_attempt_consumed(),
            // The first retained pre-plan result makes recall newly available.
            // Its schema is a real surface transition, even before a plan exists.
            recall_available: self.recall_available(),
        }
    }

    pub(super) fn projected_tools(
        &self,
        all_tools: &[PromptToolDefinition],
    ) -> Vec<PromptToolDefinition> {
        let mut tools = self.plan_policy.tools(all_tools);
        if !self.plan_policy.final_response_ready()
            && !tools.iter().any(|tool| tool.name == "orchestration.recall")
        {
            tools.extend(
                all_tools
                    .iter()
                    .filter(|tool| tool.name == "orchestration.recall")
                    .cloned(),
            );
        }
        self.filter_unavailable_tools(&mut tools);
        tools
    }

    pub(super) fn filter_unavailable_tools(&self, tools: &mut Vec<PromptToolDefinition>) {
        if !self.recall_available() {
            tools.retain(|tool| tool.name != "orchestration.recall");
        }
        if self.operation_ledger.x402_attempt_consumed() {
            tools.retain(|tool| tool.name != "x402.request");
        }
    }

    fn available_catalogue_index(
        &self,
        all_tools: &[PromptToolDefinition],
    ) -> Vec<app_core::ToolCategoryIndexEntry> {
        // Discovery metadata describes capabilities, not this round's callable
        // schemas. Apply genuine availability guards before advertising names.
        let mut available = all_tools.to_vec();
        self.filter_unavailable_tools(&mut available);
        tool_category_index(&available)
    }

    pub(super) fn admit_stable_execution_prefix(
        &mut self,
        tools: &[PromptToolDefinition],
    ) -> Result<(), ChatTerminal> {
        if self.plan_policy.active() && !self.plan_policy.uses_advisory_stages() {
            return Ok(());
        }
        if self.plan_policy.final_response_ready() {
            return if tools.is_empty() {
                Ok(())
            } else {
                Err(terminal_error(
                    IpcError {
                        code: IpcErrorCode::Internal,
                        message: "The completed execution plan retained an active tool surface unexpectedly. You can retry the request."
                            .to_owned(),
                        retryable: true,
                    },
                    GenerationFailureCategory::PromptPreparation,
                ))
            };
        }
        let fingerprint = execution_prefix_sha256(tools);
        let boundary = self.provider_prefix_boundary();
        match &self.stable_execution_prefix {
            Some(expected)
                if expected.boundary == boundary
                    && expected.sha256 != fingerprint => Err(terminal_error(
                IpcError {
                    code: IpcErrorCode::Internal,
                    message: "The stable execution prompt prefix changed unexpectedly. You can retry the request."
                        .to_owned(),
                    retryable: true,
                },
                GenerationFailureCategory::PromptPreparation,
            )),
            Some(expected)
                if expected.boundary == boundary => Ok(()),
            Some(_) | None => {
                self.stable_execution_prefix = Some(StableExecutionPrefix {
                    boundary,
                    sha256: fingerprint,
                });
                Ok(())
            }
        }
    }

    pub(super) fn admit_inference(
        &mut self,
        conversation: &Conversation,
        tools: &[PromptToolDefinition],
        projected_image_count: usize,
        execution_profile: ExecutionProfileDecision,
    ) -> Result<(), ChatTerminal> {
        let fingerprint = serde_json::json!({
            "planRevision": self.plan_policy.revision(),
            "usefulResultSequence": self.operation_ledger.useful_result_sequence(),
            "messageCount": conversation.messages.len(),
            "lastMessageId": conversation.messages.last().map(|message| &message.id),
            "imageCount": projected_image_count,
            "tools": tools.iter().map(|tool| tool.name.as_str()).collect::<Vec<_>>(),
            "profile": execution_profile.profile.diagnostic_name(),
            "work": execution_profile.work.diagnostic_name(),
            "boundedAction": execution_profile.bounded_action,
            "lengthRecovery": self.outcome_recovery_from_length,
            "boundedStage": self.active_stage_contract.as_ref().map(BoundedStageContract::operation),
        })
        .to_string();
        match self.recovery.admit_inference(fingerprint) {
            RecoveryDecision::Resume => Ok(()),
            RecoveryDecision::Terminal(error) => Err(terminal_error(
                error,
                GenerationFailureCategory::OrchestrationPhase,
            )),
        }
    }

    pub(super) fn observe_recovery_checkpoint(&mut self) {
        self.recovery.observe_checkpoint(RecoveryCheckpoint {
            plan_revision: self.plan_policy.revision(),
            forecast_revision: self.plan_policy.forecast_revision(),
            useful_result_sequence: self.operation_ledger.useful_result_sequence(),
        });
    }

    pub(super) fn request_recovery(
        &mut self,
        kind: RecoveryKind,
        scope: impl Into<String>,
        input_fingerprint: impl Into<String>,
        diagnostic: impl Into<String>,
    ) -> Result<(), ChatTerminal> {
        match self
            .recovery
            .request(kind, scope, input_fingerprint, diagnostic)
        {
            RecoveryDecision::Resume => Ok(()),
            RecoveryDecision::Terminal(error) => Err(terminal_error(
                error,
                GenerationFailureCategory::OrchestrationPhase,
            )),
        }
    }

    pub(super) fn request_transient_recovery(
        &mut self,
        kind: RecoveryKind,
        scope: impl Into<String>,
        operation_fingerprint: impl Into<String>,
        diagnostic: impl Into<String>,
    ) -> Result<(), ChatTerminal> {
        match self
            .recovery
            .request_transient(kind, scope, operation_fingerprint, diagnostic)
        {
            RecoveryDecision::Resume => Ok(()),
            RecoveryDecision::Terminal(error) => Err(terminal_error(
                error,
                GenerationFailureCategory::OrchestrationPhase,
            )),
        }
    }

    pub(super) fn request_plan_recovery(
        &mut self,
        scope: impl Into<String>,
        defects: Vec<(String, String)>,
        diagnostic: impl Into<String>,
    ) -> Result<(), ChatTerminal> {
        match self
            .recovery
            .request_plan_defects(scope, defects, diagnostic)
        {
            RecoveryDecision::Resume => Ok(()),
            RecoveryDecision::Terminal(error) => Err(terminal_error(
                error,
                GenerationFailureCategory::OrchestrationPhase,
            )),
        }
    }

    pub(super) const fn plan_defect_progress(&self) -> Option<PlanDefectProgress> {
        self.recovery.plan_defect_progress()
    }

    pub(super) fn select_bounded_stage_contract(
        &mut self,
        all_tools: &[PromptToolDefinition],
        workspace_mutation_tools_available: bool,
    ) {
        let candidate = self
            .plan_policy
            .bounded_stage_contract(all_tools, workspace_mutation_tools_available);
        self.active_stage_contract = candidate.filter(|contract| {
            self.bounded_stage_failures
                .get(&self.bounded_stage_key(contract))
                .copied()
                .unwrap_or_default()
                < BoundedStageContract::retry_limit()
        });
    }

    pub(super) fn suspend_bounded_stage(&mut self) {
        if let Some(contract) = self.active_stage_contract.take() {
            let key = self.bounded_stage_key(&contract);
            let attempts = self.bounded_stage_failures.entry(key).or_default();
            *attempts = attempts.saturating_add(1);
        }
    }

    pub(super) fn bounded_stage_key(&self, contract: &BoundedStageContract) -> String {
        format!("{}:{}", self.plan_policy.revision(), contract.operation())
    }

    pub(super) fn begin_outcome_recovery(
        &mut self,
        reason: GenerationCompletion,
        image_ids: &[ipc_types::AssetId],
    ) {
        self.outcome_recovery_used = true;
        self.outcome_recovery_attempts = self.outcome_recovery_attempts.saturating_add(1);
        self.outcome_recovery_from_length = reason == GenerationCompletion::Length;
        if self.outcome_recovery_from_length {
            self.rich_input_presentations
                .request_recovery_images(image_ids);
        }
    }

    pub(super) fn execution_profile(
        &self,
        conversation: &Conversation,
        image_count: usize,
        rich_input_reinspection: bool,
    ) -> ExecutionProfileDecision {
        // Retaining source bytes is not a request for new visual work during
        // the one post-write finish-or-continue decision. A new inspection is.
        let image_count = if self.post_write_decision_pending
            && !rich_input_reinspection
            && self.operation_ledger.pending_image_ids().next().is_none()
        {
            0
        } else {
            image_count
        };
        let unfinished = self.plan_policy.current_step_unfinished_tools();
        let unfinished_call_count = self.plan_policy.unfinished_call_count();
        let work = if !self.plan_policy.active() {
            // The fixed initial surface lets the model either retrieve bounded
            // web evidence directly or opt into semantic planning. Keep this
            // ordinary information-retrieval decision on the latency profile.
            ExecutionWorkKind::Evidence
        } else if self.plan_policy.correction_pending() || self.outcome_recovery_used {
            ExecutionWorkKind::Recovery
        } else if self.plan_policy.forecast_required() {
            ExecutionWorkKind::Planning
        } else if self.plan_policy.final_response_ready() {
            ExecutionWorkKind::FinalResponse
        } else if (self.plan_policy.forecast_complete()
            && self.plan_policy.requires_outcome_recovery())
            || self.plan_policy.current_step_is_workspace_write()
            || unfinished.iter().any(|tool| {
                matches!(
                    *tool,
                    "filesystem.write_text"
                        | "filesystem.replace_text"
                        | "filesystem.create_directory"
                        | "filesystem.write_delimited_table"
                        | "orchestration.transform_artifact"
                )
            })
        {
            ExecutionWorkKind::Mutation
        } else if self.plan_policy.current_step_is_compute()
            || unfinished
                .iter()
                .any(|tool| tool.starts_with("computation."))
        {
            ExecutionWorkKind::Transformation
        } else {
            ExecutionWorkKind::Evidence
        };
        let deviations = self
            .plan_policy
            .deviation_count()
            .saturating_add(self.argument_corrections)
            .saturating_add(self.registry_corrections)
            .saturating_add(self.phase_correction_scopes.values().sum());
        let risk = if self.outcome_recovery_from_length {
            ExecutionRisk::Ambiguous
        } else if work == ExecutionWorkKind::Mutation {
            ExecutionRisk::OutcomeBearing
        } else {
            ExecutionRisk::Routine
        };
        let decision = ExecutionProfileDecision::select(ExecutionProfileSignals {
            work,
            risk,
            complexity: ExecutionComplexity {
                message_count: conversation.messages.len(),
                image_count,
                rich_input_reinspection,
                unfinished_calls: unfinished_call_count,
                deviations,
            },
        });
        constrain_action_profile(
            decision,
            self.outcome_recovery_from_length,
            image_count == 0
                && (self.plan_policy.stage_completion_decision_due()
                    || self.post_write_decision_pending),
        )
    }

    pub(super) fn execution_state(
        &self,
        workspace_mutation_tools_available: bool,
    ) -> ToolExecutionState {
        if !self.plan_policy.active() {
            return ToolExecutionState::CurrentStepExecution;
        }
        if self.plan_policy.forecast_required() {
            return ToolExecutionState::ExecutionForecast;
        }
        if self.plan_policy.expects_file_mutation() && !workspace_mutation_tools_available {
            return ToolExecutionState::AwaitingUser;
        }
        if self.plan_policy.final_response_ready() {
            return ToolExecutionState::Completion;
        }
        if self.plan_policy.revision_recovery_pending() {
            return ToolExecutionState::PlanRevision;
        }
        if self.plan_policy.correction_pending() || self.outcome_recovery_used {
            return ToolExecutionState::SameStepRepair;
        }
        if self.plan_policy.refinement_active() {
            return ToolExecutionState::BoundedRefinement;
        }
        ToolExecutionState::CurrentStepExecution
    }

    pub(super) fn context(
        &self,
        round: usize,
        suggested_execution_reserve: usize,
        workspace_mutation_tools_available: bool,
        execution_profile: ExecutionProfileDecision,
        all_tools: &[PromptToolDefinition],
        active_tools: &[PromptToolDefinition],
    ) -> ToolOrchestrationContext {
        let mut relevant_tools = if !self.plan_policy.active() {
            active_tools.iter().map(|tool| tool.name.as_str()).collect()
        } else if self.plan_policy.forecast_required() {
            vec![FORECAST_TOOL]
        } else if let Some(contract) = &self.active_stage_contract {
            vec![contract.operation()]
        } else if self.plan_policy.active() {
            self.plan_policy.current_step_relevant_tools(all_tools)
        } else {
            vec!["orchestration.submit_plan"]
        };
        if self.active_stage_contract.is_none() {
            for tool in self.plan_policy.post_completion_relevant_tools(all_tools) {
                if !relevant_tools.contains(&tool) {
                    relevant_tools.push(tool);
                }
            }
        }
        if self.active_stage_contract.is_none() {
            for tool in self.plan_policy.safe_read_escape_tools() {
                if !relevant_tools.contains(&tool) {
                    relevant_tools.push(tool);
                }
            }
        }
        if !self.plan_policy.forecast_required()
            && self.plan_policy.forecast_revision() > 0
            && !relevant_tools.contains(&FORECAST_TOOL)
        {
            relevant_tools.push(FORECAST_TOOL);
        }
        if !self.plan_policy.forecast_required()
            && self.plan_policy.revision_available()
            && !relevant_tools.contains(&"orchestration.submit_plan")
        {
            relevant_tools.push("orchestration.submit_plan");
        }
        if self.plan_policy.uses_advisory_stages() && !relevant_tools.contains(&SEARCH_TOOLS_TOOL) {
            relevant_tools.push(SEARCH_TOOLS_TOOL);
        }
        if self.operation_ledger.x402_attempt_consumed() {
            relevant_tools.retain(|tool| *tool != "x402.request");
        }
        let scope = if self.plan_policy.final_response_ready() {
            ToolPhaseScope::FinalResponseReady
        } else if self.active_stage_contract.is_some() || self.plan_policy.execution_scope_locked()
        {
            ToolPhaseScope::ExecutionReady
        } else {
            ToolPhaseScope::Adaptive {
                revision_available: self.plan_policy.revision_available(),
            }
        };
        let category_index = self.available_catalogue_index(all_tools);
        ToolOrchestrationContext::new(
            ToolOperationBudget::new(
                round.min(MAX_CHAT_TOOL_ROUNDS),
                MAX_CHAT_TOOL_ROUNDS.saturating_sub(round),
                self.total_calls,
                MAX_CHAT_TOOL_CALLS.saturating_sub(self.total_calls),
                self.consecutive_discovery_calls,
                suggested_execution_reserve,
            ),
            workspace_mutation_tools_available,
            if self.plan_policy.active() {
                ToolOrchestrationPhase::Execution
            } else {
                ToolOrchestrationPhase::WebAnswer
            },
        )
        .with_execution_state(self.execution_state(workspace_mutation_tools_available))
        .with_plan_progress(
            self.plan_policy
                .progress_projection(self.outcome_recovery_used)
                .map(|progress| {
                    progress.with_length_exhausted_recovery(self.outcome_recovery_from_length)
                }),
        )
        .with_phase_directive(
            execution_profile,
            active_tools,
            &category_index,
            &relevant_tools,
            self.outcome_recovery_from_length && self.plan_policy.revision_available(),
            scope,
        )
        .with_recovery_diagnostic(self.pending_recovery_diagnostic())
        .with_bounded_stage_contract(
            self.active_stage_contract
                .as_ref()
                .map(BoundedStageContract::projection),
        )
    }

    pub(super) fn record_calls(&mut self, calls: &[ParsedToolCall]) {
        self.total_calls = self
            .total_calls
            .saturating_add(self.budgeted_call_count(calls));
        self.consecutive_discovery_calls =
            next_consecutive_discovery_calls(self.consecutive_discovery_calls, calls);
    }

    pub(super) fn record_results(
        &mut self,
        calls: &[ParsedToolCall],
        exchanges: &[Result<crate::chat_tools::ChatToolExchange, IpcError>],
    ) {
        let stage_contract = self.active_stage_contract.clone();
        let admission_records = self.plan_policy.active().then(|| {
            self.plan_policy.admission_records_with_stage_contract(
                calls,
                exchanges,
                stage_contract.as_ref(),
            )
        });
        if self.plan_policy.active() {
            self.plan_policy.record_results_with_stage_contract(
                calls,
                exchanges,
                stage_contract.as_ref(),
            );
        } else {
            self.preplan_result_call_ids.extend(
                calls
                    .iter()
                    .zip(exchanges)
                    .filter(|(_, exchange)| {
                        exchange.as_ref().is_ok_and(|exchange| {
                            matches!(exchange.result.outcome, ToolOutcome::Completed { .. })
                        })
                    })
                    .map(|(call, _)| call.call_id.clone()),
            );
        }
        self.operation_ledger.record_results(calls, exchanges);
        if let Some(admission_records) = admission_records {
            self.operation_ledger.record_admissions(admission_records);
        }
        if calls.iter().any(|call| {
            !matches!(
                call.name.as_str(),
                "orchestration.submit_plan"
                    | "orchestration.complete_stage"
                    | "orchestration.submit_forecast"
                    | "orchestration.search_tools"
            )
        }) {
            self.post_write_decision_pending = self.plan_policy.uses_advisory_stages()
                && file_content_mutation_completed(exchanges)
                && crate::chat_tools::applied_changes_verified(exchanges);
        }
        if exchanges.iter().any(|exchange| {
            exchange.as_ref().is_ok_and(|exchange| {
                matches!(&exchange.result.outcome, ToolOutcome::Completed { .. })
            })
        }) {
            self.outcome_recovery_used = false;
            self.outcome_recovery_from_length = false;
        }
        if let Some(contract) = stage_contract {
            let satisfied = calls
                .iter()
                .zip(exchanges)
                .any(|(call, exchange)| contract.postcondition_satisfied(call, exchange));
            if !satisfied {
                let key = self.bounded_stage_key(&contract);
                let attempts = self.bounded_stage_failures.entry(key).or_default();
                *attempts = attempts.saturating_add(1);
            }
            self.active_stage_contract = None;
        }
    }

    pub(super) fn retain_preplan_results(&mut self) {
        self.plan_policy
            .retain_context_results(self.preplan_result_call_ids.drain(..));
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn trace_and_record_results(
        &mut self,
        core: &AppCore,
        request_id: &RequestId,
        conversation: &Conversation,
        round: usize,
        calls: &[ParsedToolCall],
        exchanges: &[Result<crate::chat_tools::ChatToolExchange, IpcError>],
    ) {
        trace_plan_tool_results(
            core,
            request_id,
            conversation,
            round,
            &self.plan_policy,
            calls,
            exchanges,
        );
        self.record_results(calls, exchanges);
    }
}

pub(super) fn execution_prefix_sha256(tools: &[PromptToolDefinition]) -> String {
    let mut hasher = Sha256::new();
    for tool in tools {
        let schema = tool.parameters.to_string();
        for component in [
            tool.name.as_bytes(),
            tool.description.as_bytes(),
            schema.as_bytes(),
        ] {
            hasher.update(component.len().to_le_bytes());
            hasher.update(component);
        }
    }
    format!("{:x}", hasher.finalize())
}

pub(super) fn constrain_action_profile(
    decision: ExecutionProfileDecision,
    output_exhausted: bool,
    stage_decision_due: bool,
) -> ExecutionProfileDecision {
    if output_exhausted || stage_decision_due {
        decision.for_bounded_action()
    } else {
        decision
    }
}

pub(super) fn output_exhaustion_recovery_diagnostic(plan_policy: &PlanPolicy) -> &'static str {
    if plan_policy.uses_advisory_stages() {
        if plan_policy.current_step_is_workspace_write() {
            return "The prior action round exhausted its output allowance before emitting a call. Reuse retained evidence and emit the exposed dependency-ready write now. Put complete extracted rows directly in structured write arguments; do not transcribe, recalculate or narrate them in private reasoning.";
        }
        return "The prior action round exhausted its output allowance before emitting a call. Reuse retained evidence and inspect the active doneWhen condition before reasoning further: if it is satisfied, emit orchestration.complete_stage now; otherwise emit one suitable task call for a specific missing fact. Do not repeat transcription, arithmetic or established analysis in private reasoning. When the later write stage is ready, put complete extracted rows directly in structured write arguments.";
    }
    "The prior action round exhausted its output allowance before emitting the required operation. Resume this same step with retained evidence and emit the outstanding structured call without repeating prior analysis."
}
