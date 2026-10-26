//! Focused plan-policy responsibility extracted from the feature coordinator.

//! Execution-result reconciliation and verified plan progress.

use super::{
    BoundedStageContract, COMPLETE_STAGE_TOOL, ChatToolExchange, CompletedEvidence, EvidenceClass,
    EvidenceProvision, EvidenceRole, FORECAST_TOOL, ForecastDiagnosticMatch, ForecastStep,
    IpcError, MAX_EVIDENCE_REFINEMENTS_PER_REVISION, MAX_RETAINED_ADVISORY_RESULTS, PLAN_TOOL,
    ParsedToolCall, PlanActionClass, PlanContractVersion, PlanEvidenceClass, PlanEvidenceRoleKind,
    PlanPolicy, PlannedOutcome, TaskActivityStatus, ToolName, ToolOutcome, ToolPlanProgress,
    ToolPlanStepProgress, ToolPlanStepStatus, ToolStagePostcondition, evidence_class_name,
    exchange_satisfies_forecast, invalid_arguments, is_evidence_read_tool, policy_error,
    retain_recent_unique, stage_postcondition_satisfied, terminal_result_completes_stage,
    verified_obligation_receipts,
};

impl PlanPolicy {
    pub(in crate::chat_runtime) fn retain_context_results(
        &mut self,
        call_ids: impl IntoIterator<Item = String>,
    ) {
        retain_recent_unique(
            &mut self.retained_advisory_call_ids,
            call_ids,
            MAX_RETAINED_ADVISORY_RESULTS,
        );
    }

    #[cfg(test)]
    pub(in crate::chat_runtime) fn retains_context_result(&self, call_id: &str) -> bool {
        self.retained_advisory_call_ids
            .iter()
            .any(|retained| retained == call_id)
    }

    #[cfg(test)]
    pub(in crate::chat_runtime) fn record_results(
        &mut self,
        calls: &[ParsedToolCall],
        exchanges: &[Result<ChatToolExchange, IpcError>],
    ) {
        self.record_results_with_stage_contract(calls, exchanges, None);
    }

    pub(in crate::chat_runtime) fn record_results_with_stage_contract(
        &mut self,
        calls: &[ParsedToolCall],
        exchanges: &[Result<ChatToolExchange, IpcError>],
        stage_contract: Option<&BoundedStageContract>,
    ) {
        let charged = exchanges
            .iter()
            .filter_map(|exchange| exchange.as_ref().ok())
            .filter(|exchange| {
                exchange.request.name != ToolName::OrchestrationSubmitPlan
                    && exchange.request.name != ToolName::OrchestrationSubmitForecast
                    && exchange.request.name != ToolName::OrchestrationCompleteStage
                    && exchange.activity.status != TaskActivityStatus::Reused
            })
            .count();
        self.operations_used = self.operations_used.saturating_add(charged);
        let mut returned_to_forecast = false;
        let mut observed_deviation = None;
        for (call, exchange) in calls.iter().zip(exchanges) {
            if call.name == PLAN_TOOL
                || call.name == FORECAST_TOOL
                || call.name == COMPLETE_STAGE_TOOL
            {
                continue;
            }
            let (returned, deviation) = self.record_single_result(call, exchange, stage_contract);
            returned_to_forecast |= returned;
            observed_deviation = deviation.or(observed_deviation);
        }
        if observed_deviation.is_some() {
            self.pending_deviation = observed_deviation;
        } else if returned_to_forecast {
            self.pending_deviation = None;
        }
        self.record_outcome_results(exchanges);
        self.clear_completed_safe_read_escapes(calls, exchanges);
    }

    pub(in crate::chat_runtime) fn record_single_result(
        &mut self,
        call: &ParsedToolCall,
        exchange: &Result<ChatToolExchange, IpcError>,
        stage_contract: Option<&BoundedStageContract>,
    ) -> (bool, Option<ForecastDiagnosticMatch>) {
        if self
            .contract_version
            .is_some_and(PlanContractVersion::uses_advisory_stages)
        {
            return (self.record_advisory_stage_result(call, exchange), None);
        }
        let diagnostic = self.diagnostic_match_unconditional(call);
        let matched = diagnostic == ForecastDiagnosticMatch::Matched;
        let required_provisions = self.matching_forecast_provisions(call);
        let semantic_step_index = self.matching_ready_step_index(call);
        let semantic_step = semantic_step_index.map(|index| &self.forecast_steps[index]);
        let semantic_match = semantic_step.is_some_and(|step| step.matches_semantic_tool(call));
        let transport_completed = exchange
            .as_ref()
            .is_ok_and(|exchange| matches!(exchange.result.outcome, ToolOutcome::Completed { .. }))
            && stage_contract.is_none_or(|contract| {
                !contract.matches(call) || contract.postcondition_satisfied(call, exchange)
            });
        let semantic_evidence_satisfied = semantic_step.is_some_and(|step| {
            step.semantic_evidence_satisfied_by(call)
                && (step.action_class != Some(PlanActionClass::WorkspaceWrite)
                    || !self.expects_file_mutation()
                    || self.is_outcome_bearing_call(call))
        });
        let satisfies_forecast = transport_completed
            && exchange.as_ref().is_ok_and(|exchange| {
                exchange_satisfies_forecast(call, exchange, &required_provisions)
                    && (!semantic_match || semantic_evidence_satisfied)
                    && semantic_step.is_none_or(|step| {
                        step.semantic_obligations.is_empty()
                            || verified_obligation_receipts(
                                call,
                                exchange,
                                &step.output_obligations(),
                            )
                            .is_some()
                    })
            });
        self.record_evidence_attempt(call, matched, transport_completed, satisfies_forecast);
        if satisfies_forecast && self.complete_forecast_call(call, exchange, semantic_step_index) {
            return (true, None);
        }
        if !self
            .contract_version
            .is_some_and(PlanContractVersion::uses_typed_obligations)
            && semantic_match
            && transport_completed
            && self.record_semantic_adaptation(call)
        {
            return (true, None);
        }
        if !satisfies_forecast && self.block_forecast_call(call) {
            return (matched, None);
        }
        self.unforecasted_operations = self.unforecasted_operations.saturating_add(1);
        (false, Some(diagnostic))
    }

    pub(in crate::chat_runtime) fn record_advisory_stage_result(
        &mut self,
        call: &ParsedToolCall,
        exchange: &Result<ChatToolExchange, IpcError>,
    ) -> bool {
        let ready = self.ready_step_indices();
        let Some(index) = ready
            .iter()
            .copied()
            .find(|index| {
                self.forecast_steps[*index]
                    .calls
                    .iter()
                    .any(|suggestion| suggestion.tool == call.name)
            })
            .or_else(|| {
                ready.into_iter().find(|index| {
                    let step = &self.forecast_steps[*index];
                    step.action_class
                        .is_some_and(|class| class.matches_tool(&call.name))
                })
            })
            .or_else(|| self.primary_ready_step_index())
        else {
            // Tools remain usable after a write-derived cursor advance. Retain
            // subsequent returned results even when no advisory stage is current;
            // otherwise finish-or-continue would offer tools but forget their work.
            if exchange.as_ref().is_ok_and(|exchange| {
                matches!(exchange.result.outcome, ToolOutcome::Completed { .. })
            }) {
                retain_recent_unique(
                    &mut self.retained_advisory_call_ids,
                    [call.call_id.clone()],
                    MAX_RETAINED_ADVISORY_RESULTS,
                );
            }
            return false;
        };
        let Some(exchange) = exchange.as_ref().ok() else {
            return false;
        };
        if terminal_result_completes_stage(&call.name, &call.arguments, &exchange.result.outcome)
            && exchange.activity.status != TaskActivityStatus::Partial
        {
            // The operation contract defines every authoritative first result
            // as terminal for this user-bounded attempt. Treat that returned
            // result as the stage's semantic boundary so a failure is reported
            // instead of causing a retry, tool search, or redundant revision.
            let step = &mut self.forecast_steps[index];
            step.blocked = false;
            if !step.semantic_completed_call_ids.contains(&call.call_id) {
                step.semantic_completed_call_ids.push(call.call_id.clone());
            }
            step.semantic_complete = true;
            step.semantic_completion_declared = true;
            return true;
        }
        if !matches!(exchange.result.outcome, ToolOutcome::Completed { .. }) {
            return false;
        }
        let workspace_write =
            self.forecast_steps[index].action_class == Some(PlanActionClass::WorkspaceWrite);
        let outcome_bearing = self.is_outcome_bearing_call(call);
        {
            let step = &mut self.forecast_steps[index];
            step.blocked = false;
            if !step.semantic_completed_call_ids.contains(&call.call_id) {
                step.semantic_completed_call_ids.push(call.call_id.clone());
            }
        }
        // Read and compute completion is a semantic judgment made explicitly by
        // the model through orchestration.complete_stage. Only a host-verified
        // outcome-bearing write may advance without that declaration.
        if workspace_write
            && outcome_bearing
            && self.has_single_advisory_write_stage()
            && exchange_satisfies_forecast(call, exchange, &[])
        {
            self.forecast_steps[index].semantic_complete = true;
            return true;
        }
        false
    }

    pub(in crate::chat_runtime) fn complete_advisory_stage(
        &mut self,
        call: &ParsedToolCall,
    ) -> Result<serde_json::Value, IpcError> {
        let index = self.validate_advisory_stage_completion(call)?;
        let stage_id = self.forecast_steps[index].id.clone();
        self.forecast_steps[index].semantic_complete = true;
        self.forecast_steps[index].semantic_completion_declared = true;
        self.forecast_steps[index].blocked = false;
        self.reconcile_verified_advisory_outcome();
        let next_stage_id = self
            .primary_ready_step_index()
            .map(|next| self.forecast_steps[next].id.clone());
        Ok(serde_json::json!({
            "accepted": true,
            "contractVersion": "semantic_execution_v6",
            "stageId": stage_id,
            "status": "complete",
            "nextStageId": next_stage_id,
            "nextStage": self.primary_ready_step_index().map(|next| {
                let step = &self.forecast_steps[next];
                serde_json::json!({"id": step.id, "objective": step.purpose})
            }),
            "verifiedWriteCallIds": self.verified_outcome_call_ids,
            "continuation": "The declaration acknowledges only stageId. If returned results already satisfy the overall target and success criteria, give the final answer; no housekeeping completion calls are needed. Otherwise continue the remaining work. A verified write alone does not prove every deliverable is complete.",
            "permissionGranted": false
        }))
    }

    pub(in crate::chat_runtime) fn validate_advisory_stage_completion(
        &self,
        _call: &ParsedToolCall,
    ) -> Result<usize, IpcError> {
        if self.contract_version != Some(PlanContractVersion::SemanticExecutionV6) {
            return Err(policy_error(
                "orchestration.complete_stage is available only for an active semantic_execution_v6 plan",
            ));
        }
        let index = self
            .primary_ready_step_index()
            .ok_or_else(|| policy_error("No dependency-ready incomplete stage is current"))?;
        Ok(index)
    }

    pub(in crate::chat_runtime) fn record_evidence_attempt(
        &mut self,
        call: &ParsedToolCall,
        matched: bool,
        transport_completed: bool,
        satisfies_forecast: bool,
    ) {
        if !matched || !is_evidence_read_tool(&call.name) || !transport_completed {
            return;
        }
        let matching_step_id = self
            .matching_ready_step_index(call)
            .map(|index| self.forecast_steps[index].id.clone());
        let candidate_key = matching_step_id
            .clone()
            .unwrap_or_else(|| format!("operation:{}", call.name));
        if satisfies_forecast {
            self.unresolved_evidence_call_ids.remove(&candidate_key);
            if self
                .refinement_step_id
                .as_ref()
                .is_some_and(|id| matching_step_id.as_ref() == Some(id))
            {
                self.refinement_step_id = None;
            }
            return;
        }
        self.unresolved_evidence_call_ids
            .insert(candidate_key, call.call_id.clone());
        if matching_step_id.is_some() {
            self.refinement_step_id = matching_step_id;
        }
        if self.evidence_refinements_used < MAX_EVIDENCE_REFINEMENTS_PER_REVISION {
            self.evidence_refinements_used = self.evidence_refinements_used.saturating_add(1);
            self.working_allowance = self.working_allowance.saturating_add(1);
        }
    }

    pub(in crate::chat_runtime) fn record_outcome_results(
        &mut self,
        exchanges: &[Result<ChatToolExchange, IpcError>],
    ) {
        let outcome = self.outcome;
        let verified_call_ids = exchanges
            .iter()
            .filter_map(|exchange| exchange.as_ref().ok())
            .filter(|exchange| match outcome {
                Some(PlannedOutcome::CreateFile) => {
                    matches!(
                        exchange.request.name,
                        ToolName::FilesystemWriteText
                            | ToolName::FilesystemWriteDelimitedTable
                            | ToolName::OrchestrationTransformArtifact
                    ) && stage_postcondition_satisfied(
                        ToolStagePostcondition::FileWriteVerified,
                        exchange,
                    )
                }
                Some(PlannedOutcome::ModifyFile) => {
                    matches!(
                        exchange.request.name,
                        ToolName::FilesystemWriteText
                            | ToolName::FilesystemReplaceText
                            | ToolName::FilesystemWriteDelimitedTable
                            | ToolName::OrchestrationTransformArtifact
                    ) && stage_postcondition_satisfied(
                        ToolStagePostcondition::FileWriteVerified,
                        exchange,
                    )
                }
                Some(PlannedOutcome::Other) => {
                    matches!(
                        exchange.request.name,
                        ToolName::FilesystemWriteText
                            | ToolName::FilesystemReplaceText
                            | ToolName::FilesystemWriteDelimitedTable
                    ) && stage_postcondition_satisfied(
                        ToolStagePostcondition::FileWriteVerified,
                        exchange,
                    )
                }
                Some(PlannedOutcome::Answer) | None => false,
            })
            .map(|exchange| exchange.result.call_id.0.clone())
            .collect::<Vec<_>>();
        if !verified_call_ids.is_empty() {
            self.outcome_satisfied = true;
            self.outcome_execution_reserve_available = false;
            retain_recent_unique(
                &mut self.verified_outcome_call_ids,
                verified_call_ids,
                MAX_RETAINED_ADVISORY_RESULTS,
            );
        }
    }

    /// Reconciles an already-authorized and host-verified outcome with a write
    /// stage that became dependency-ready at an explicit completion boundary.
    /// This grants no authority and never infers completion of a read or compute
    /// stage; it only prevents a successful effect from being discarded because
    /// the model placed it immediately before the boundary.
    pub(in crate::chat_runtime) fn reconcile_verified_advisory_outcome(&mut self) {
        if !self
            .contract_version
            .is_some_and(PlanContractVersion::uses_advisory_stages)
            || !self.outcome_satisfied
            || self.verified_outcome_call_ids.is_empty()
            || !self.has_single_advisory_write_stage()
        {
            return;
        }
        let Some(index) = self.primary_ready_step_index() else {
            return;
        };
        if self.forecast_steps[index].action_class != Some(PlanActionClass::WorkspaceWrite) {
            return;
        }
        let verified = self.verified_outcome_call_ids.clone();
        let step = &mut self.forecast_steps[index];
        for call_id in verified {
            if !step.semantic_completed_call_ids.contains(&call_id) {
                step.semantic_completed_call_ids.push(call_id);
            }
        }
        step.semantic_complete = true;
        step.blocked = false;
    }

    pub(in crate::chat_runtime) fn clear_completed_safe_read_escapes(
        &mut self,
        calls: &[ParsedToolCall],
        exchanges: &[Result<ChatToolExchange, IpcError>],
    ) {
        for (call, exchange) in calls.iter().zip(exchanges) {
            if self.pending_safe_read_escapes.contains(&call.name)
                && exchange.as_ref().is_ok_and(|exchange| {
                    matches!(exchange.result.outcome, ToolOutcome::Completed { .. })
                })
            {
                self.pending_safe_read_escapes.remove(&call.name);
            }
        }
    }

    pub(in crate::chat_runtime) fn reopen_completed_read(
        &mut self,
        operation: &ParsedToolCall,
    ) -> bool {
        let current = self
            .forecast_steps
            .iter()
            .position(ForecastStep::unfinished)
            .unwrap_or(self.forecast_steps.len());
        let candidate = self.forecast_steps[..current]
            .iter()
            .enumerate()
            .rev()
            .find_map(|(step_index, step)| {
                step.calls
                    .iter()
                    .find(|call| call.completed && call.matches(operation))
                    .and_then(|call| {
                        call.completed_call_id
                            .as_ref()
                            .map(|call_id| (step_index, call_id.clone()))
                    })
            });
        let Some((step_index, completed_call_id)) = candidate else {
            return false;
        };
        let step = &mut self.forecast_steps[step_index];
        for call in &mut step.calls {
            if call.completed_call_id.as_deref() == Some(completed_call_id.as_str()) {
                call.completed = false;
                call.completed_call_id = None;
            }
        }
        step.blocked = false;
        step.satisfied_obligations
            .retain(|_, receipt| receipt.call_id != completed_call_id);
        step.semantic_completed_call_ids
            .retain(|call_id| call_id != &completed_call_id);
        step.semantic_complete = step.complete();
        self.completed_evidence
            .retain(|evidence| evidence.call_id != completed_call_id);
        self.pending_deviation = None;
        true
    }

    pub(in crate::chat_runtime) fn complete_forecast_call(
        &mut self,
        operation: &ParsedToolCall,
        exchange: &Result<ChatToolExchange, IpcError>,
        matched_index: Option<usize>,
    ) -> bool {
        let Some(index) = matched_index.or_else(|| self.matching_ready_step_index(operation))
        else {
            return false;
        };
        if !self.forecast_steps[index].semantic_obligations.is_empty() {
            let Some(exchange) = exchange.as_ref().ok() else {
                return false;
            };
            let outputs = self.forecast_steps[index].output_obligations();
            let Some(receipts) = verified_obligation_receipts(operation, exchange, &outputs) else {
                return false;
            };
            let step = &mut self.forecast_steps[index];
            for (obligation_id, receipt) in receipts {
                step.satisfied_obligations.insert(obligation_id, receipt);
            }
            if !step
                .semantic_completed_call_ids
                .contains(&operation.call_id)
            {
                step.semantic_completed_call_ids
                    .push(operation.call_id.clone());
            }
            step.semantic_complete = step.complete();
            step.blocked = false;
            return step.complete();
        }
        let step = &mut self.forecast_steps[index];
        if step.matches_semantic_tool(operation) {
            step.semantic_complete = true;
            step.semantic_completed_call_ids
                .push(operation.call_id.clone());
            step.blocked = false;
            return true;
        }
        let matching = step
            .calls
            .iter()
            .enumerate()
            .filter(|(_, call)| !call.completed && call.matches(operation))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let Some(first) = matching.first().copied() else {
            return false;
        };
        let is_batched = matching.len() > 1
            && matching.iter().all(|index| {
                step.calls[*index].scope.as_ref().is_some_and(|scope| {
                    operation
                        .arguments
                        .get(&scope.argument)
                        .is_some_and(|value| value.is_array() || value.is_object())
                })
            });
        let completed_indices = if is_batched {
            for index in &matching {
                step.calls[*index].completed = true;
                step.calls[*index].completed_call_id = Some(operation.call_id.clone());
            }
            matching
        } else {
            step.calls[first].completed = true;
            step.calls[first].completed_call_id = Some(operation.call_id.clone());
            vec![first]
        };
        let provisions = completed_indices
            .into_iter()
            .flat_map(|index| step.calls[index].provides_evidence.iter().copied())
            .collect::<Vec<_>>();
        step.blocked = false;
        for provision in provisions {
            let completed = CompletedEvidence {
                call_id: operation.call_id.clone(),
                provision,
            };
            if !self.completed_evidence.contains(&completed) {
                self.completed_evidence.push(completed);
            }
        }
        true
    }

    pub(in crate::chat_runtime) fn record_semantic_adaptation(
        &mut self,
        operation: &ParsedToolCall,
    ) -> bool {
        let Some(index) = self.matching_ready_step_index(operation) else {
            return false;
        };
        let step = &mut self.forecast_steps[index];
        if !step
            .semantic_completed_call_ids
            .contains(&operation.call_id)
        {
            step.semantic_completed_call_ids
                .push(operation.call_id.clone());
        }
        step.blocked = false;
        true
    }

    pub(super) fn matching_forecast_provisions(
        &self,
        operation: &ParsedToolCall,
    ) -> Vec<EvidenceProvision> {
        self.matching_ready_step_index(operation)
            .map(|index| &self.forecast_steps[index])
            .into_iter()
            .flat_map(|step| &step.calls)
            .filter(|call| !call.completed && call.matches(operation))
            .flat_map(|call| call.provides_evidence.iter().copied())
            .collect()
    }

    pub(in crate::chat_runtime) fn validate_artifact_evidence_bindings(
        &self,
        operation: &ParsedToolCall,
    ) -> Result<(), IpcError> {
        let bindings = operation
            .arguments
            .get("evidence")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(invalid_arguments)?;
        if self
            .contract_version
            .is_some_and(PlanContractVersion::uses_typed_obligations)
        {
            let index = self
                .matching_ready_step_index(operation)
                .ok_or_else(|| policy_error("Artifact operation is not dependency-ready"))?;
            let expected = self.v3_prerequisite_receipts(index).ok_or_else(|| {
                IpcError::plan_mismatch(
                    "Artifact operation is missing independently grounded typed obligations",
                )
            })?;
            for (role_name, role_kind) in [
                ("source", PlanEvidenceRoleKind::Source),
                ("convention", PlanEvidenceRoleKind::Convention),
            ] {
                let expected_call_id = expected.iter().find_map(|(id, receipt)| {
                    self.forecast_steps[index]
                        .semantic_obligations
                        .iter()
                        .find(|obligation| {
                            obligation.id == *id && obligation.role_kind == role_kind
                        })
                        .map(|_| receipt.call_id.as_str())
                });
                let bound = bindings.iter().any(|binding| {
                    binding.get("role").and_then(serde_json::Value::as_str) == Some(role_name)
                        && binding.get("callId").and_then(serde_json::Value::as_str)
                            == expected_call_id
                });
                if expected_call_id.is_none() || !bound {
                    return Err(policy_error(&format!(
                        "Artifact transformation `{role_name}` evidence must bind the exact host-verified receipt for that typed role"
                    )));
                }
            }
            return Ok(());
        }
        for (role_name, role, minimum) in [
            (
                "source",
                EvidenceRole::Source,
                EvidenceClass::SemanticContent,
            ),
            (
                "convention",
                EvidenceRole::Convention,
                EvidenceClass::FormatStructure,
            ),
        ] {
            let matching_ids = bindings.iter().filter_map(|binding| {
                let binding = binding.as_object()?;
                (binding.get("role")?.as_str()? == role_name)
                    .then(|| binding.get("callId")?.as_str())
                    .flatten()
            });
            let semantic_minimum = match minimum {
                EvidenceClass::IdentityLocation => PlanEvidenceClass::IdentityLocation,
                EvidenceClass::FormatStructure => PlanEvidenceClass::FormatStructure,
                EvidenceClass::SemanticContent => PlanEvidenceClass::SemanticContent,
            };
            let semantic_index = self.matching_ready_step_index(operation);
            if !matching_ids.into_iter().any(|call_id| {
                self.completed_evidence.iter().any(|evidence| {
                    evidence.call_id == call_id
                        && evidence.provision.role == role
                        && evidence.provision.class >= minimum
                }) || semantic_index.is_some_and(|index| {
                    self.semantic_dependency_receipt(index, semantic_minimum)
                        .as_deref()
                        == Some(call_id)
                })
            }) {
                return Err(policy_error(&format!(
                    "Artifact transformation `{role_name}` evidence must bind the exact call ID of a completed forecast provision classified as {} or stronger",
                    evidence_class_name(minimum)
                )));
            }
        }
        Ok(())
    }

    pub(in crate::chat_runtime) fn block_forecast_call(
        &mut self,
        operation: &ParsedToolCall,
    ) -> bool {
        let Some(index) = self.matching_ready_step_index(operation) else {
            return false;
        };
        let step = &mut self.forecast_steps[index];
        if step.matches_semantic_tool(operation) {
            step.blocked = true;
            return true;
        }
        if !step
            .calls
            .iter()
            .any(|call| !call.completed && call.matches(operation))
        {
            return false;
        }
        step.blocked = true;
        true
    }

    pub(in crate::chat_runtime) fn diagnostic_match_unconditional(
        &self,
        call: &ParsedToolCall,
    ) -> ForecastDiagnosticMatch {
        let Some(current) = self.primary_ready_step_index() else {
            return ForecastDiagnosticMatch::Unforecasted;
        };
        if self.forecast_steps[current].matches_operation(call)
            || self
                .ready_step_indices()
                .into_iter()
                .any(|index| index != current && self.forecast_steps[index].matches_operation(call))
        {
            return ForecastDiagnosticMatch::Matched;
        }
        if self.forecast_steps.iter().enumerate().any(|(index, step)| {
            step.unfinished()
                && !self.step_dependencies_complete(index)
                && step.matches_operation(call)
        }) {
            ForecastDiagnosticMatch::OutOfOrder
        } else {
            ForecastDiagnosticMatch::Unforecasted
        }
    }

    pub(in crate::chat_runtime) fn progress_projection(
        &self,
        recovery_required: bool,
    ) -> Option<ToolPlanProgress> {
        let outcome = self.outcome?;
        let mut retained_call_ids = self
            .unresolved_evidence_call_ids
            .values()
            .cloned()
            .collect::<Vec<_>>();
        retained_call_ids.extend(self.retained_advisory_call_ids.iter().cloned());
        retained_call_ids.extend(self.verified_outcome_call_ids.iter().cloned());
        if !self
            .contract_version
            .is_some_and(PlanContractVersion::is_semantic)
        {
            retained_call_ids.extend(
                self.completed_evidence
                    .iter()
                    .map(|evidence| evidence.call_id.clone()),
            );
        }
        let steps = self
            .forecast_steps
            .iter()
            .enumerate()
            .map(|(index, step)| {
                let completed = step.calls.iter().filter(|call| call.completed).count();
                let status = if step.complete() {
                    ToolPlanStepStatus::ForecastComplete
                } else if step.blocked {
                    ToolPlanStepStatus::Blocked
                } else if completed > 0 || step.has_completed_work() {
                    ToolPlanStepStatus::InProgress
                } else {
                    ToolPlanStepStatus::NotStarted
                };
                ToolPlanStepProgress::new(
                    index.saturating_add(1),
                    status,
                    step.calls
                        .iter()
                        .filter(|call| !step.complete() && !call.completed)
                        .map(|call| call.tool.clone())
                        .collect(),
                )
                .with_contract(
                    step.id.clone(),
                    step.purpose.clone(),
                    step.required_evidence.clone(),
                    step.completion_evidence.clone(),
                    step.calls
                        .iter()
                        .filter_map(|call| call.completed_call_id.clone())
                        .chain(step.semantic_completed_call_ids.iter().cloned())
                        .collect(),
                )
                .with_dependencies(step.depends_on.clone())
                .with_forecast_category(
                    self.execution_forecast
                        .as_ref()
                        .and_then(|forecast| {
                            forecast
                                .entries
                                .iter()
                                .find(|entry| entry.step_id == step.id)
                        })
                        .map(|entry| entry.category.clone()),
                )
            })
            .collect::<Vec<_>>();
        let suggested_step = self
            .primary_ready_step_index()
            .map(|index| index.saturating_add(1));
        let mut progress = ToolPlanProgress::new(
            self.revision,
            self.completed_before_revision,
            steps,
            suggested_step,
            outcome.into(),
            self.outcome_satisfied,
            recovery_required,
            self.working_allowance.saturating_sub(self.operations_used),
            self.outcome_execution_reserve_available,
            self.unforecasted_operations,
        )
        .with_retained_call_ids(retained_call_ids)
        .with_deviation_recovery(self.pending_deviation.is_some());
        if let Some(contract) = &self.semantic_contract {
            progress = progress.with_outcome_contract(
                contract.target.clone(),
                contract.constraints.clone(),
                contract.success_criteria.clone(),
            );
        }
        Some(progress)
    }

    pub(in crate::chat_runtime) const fn planned_outcome_satisfied(&self) -> bool {
        self.outcome_satisfied
    }

    /// An unbound write receipt cannot identify which of several semantic write
    /// stages it completes. Leave those judgments to explicit model checkpoints.
    fn has_single_advisory_write_stage(&self) -> bool {
        self.forecast_steps
            .iter()
            .filter(|step| step.action_class == Some(PlanActionClass::WorkspaceWrite))
            .count()
            == 1
    }

    pub(in crate::chat_runtime) fn allows_automatic_write_confirmation(&self) -> bool {
        !self.uses_advisory_stages() && (!self.active() || self.outcome_satisfied)
    }

    pub(in crate::chat_runtime) const fn expects_file_mutation(&self) -> bool {
        matches!(
            self.outcome,
            Some(PlannedOutcome::CreateFile | PlannedOutcome::ModifyFile)
        )
    }

    pub(in crate::chat_runtime) const fn requires_outcome_recovery(&self) -> bool {
        self.expects_file_mutation() && !self.outcome_satisfied
    }

    pub(in crate::chat_runtime) fn is_outcome_bearing_call(&self, call: &ParsedToolCall) -> bool {
        self.is_outcome_bearing_tool(&call.name)
    }

    pub(in crate::chat_runtime) fn is_outcome_bearing_tool(&self, tool: &str) -> bool {
        match self.outcome {
            Some(PlannedOutcome::CreateFile) => matches!(
                tool,
                "filesystem.write_text"
                    | "filesystem.write_delimited_table"
                    | "orchestration.transform_artifact"
            ),
            Some(PlannedOutcome::ModifyFile) => matches!(
                tool,
                "filesystem.write_text"
                    | "filesystem.replace_text"
                    | "filesystem.write_delimited_table"
                    | "orchestration.transform_artifact"
            ),
            Some(PlannedOutcome::Answer | PlannedOutcome::Other) | None => false,
        }
    }

    #[cfg(test)]
    pub(in crate::chat_runtime) const fn operations_used(&self) -> usize {
        self.operations_used
    }

    #[cfg(test)]
    pub(in crate::chat_runtime) const fn working_allowance(&self) -> usize {
        self.working_allowance
    }

    #[cfg(test)]
    pub(in crate::chat_runtime) const fn deviation_pending(&self) -> bool {
        self.pending_deviation.is_some()
    }
}
