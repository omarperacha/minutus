//! Focused plan-policy responsibility extracted from the feature coordinator.

//! Plan admission, correction, and operation-policy validation.

use super::{
    AdmittedPlan, BoundedStageContract, ChatToolExchange, EvidenceClass, EvidenceRole,
    FORECAST_TOOL, ForecastDiagnosticMatch, IpcError, MAX_CORRECTIONS_PER_REVISION,
    MAX_IDENTICAL_CORRECTIONS_PER_REVISION, MAX_PLAN_REVISIONS, MAX_RETAINED_ADVISORY_RESULTS,
    OperationAdmissionRecord, OperationPermissionOutcome, OperationProgressContribution,
    OperationStepRelationship, PLAN_RESERVE_CALLS, PLAN_TOOL, ParsedToolCall, PlanActionClass,
    PlanContractVersion, PlanPolicy, PlannedOutcome, PromptToolDefinition, SemanticPlanContract,
    ToolOutcome, collector_operations_for_obligation, exchange_satisfies_forecast,
    invalid_arguments, is_evidence_read_tool, is_safe_read_tool, operation_can_address_obligations,
    parse_semantic_steps, parse_steps, plan_contract_metadata, plan_signature, policy_error,
    postcondition_satisfies, retain_recent_unique, retain_semantic_progress,
    stage_postcondition_satisfied, string, validate_forecast_outcome,
};
use crate::chat_tools::permission_outcome;
use std::collections::BTreeSet;

impl PlanPolicy {
    pub(in crate::chat_runtime) fn initial_plan_required(&self, calls: &[ParsedToolCall]) -> bool {
        !self.submitted && calls.first().is_none_or(|call| call.name != PLAN_TOOL)
    }

    pub(in crate::chat_runtime) fn correction_available(&self, error: &IpcError) -> bool {
        self.corrections.get(&self.revision).is_none_or(|state| {
            state.total < MAX_CORRECTIONS_PER_REVISION
                && state
                    .diagnostics
                    .get(&error.message)
                    .copied()
                    .unwrap_or_default()
                    < MAX_IDENTICAL_CORRECTIONS_PER_REVISION
        })
    }

    pub(in crate::chat_runtime) fn record_correction(&mut self, error: &IpcError) {
        let state = self.corrections.entry(self.revision).or_default();
        state.total = state.total.saturating_add(1);
        let attempts = state.diagnostics.entry(error.message.clone()).or_default();
        *attempts = attempts.saturating_add(1);
        state.pending = true;
    }

    pub(in crate::chat_runtime) fn correction_pending(&self) -> bool {
        self.corrections
            .get(&self.revision)
            .is_some_and(|state| state.pending)
    }

    pub(in crate::chat_runtime) fn refinement_active(&self) -> bool {
        self.refinement_step_id.as_ref().is_some_and(|id| {
            self.forecast_steps.iter().enumerate().any(|(index, step)| {
                step.id == *id && step.unfinished() && self.step_dependencies_complete(index)
            })
        })
    }

    pub(in crate::chat_runtime) const fn revision_recovery_pending(&self) -> bool {
        self.pending_deviation.is_some()
    }

    pub(in crate::chat_runtime) fn resolve_correction(&mut self) {
        if let Some(state) = self.corrections.get_mut(&self.revision) {
            state.pending = false;
        }
    }

    pub(in crate::chat_runtime) fn correction_scope(&self) -> String {
        format!("plan:{}", self.revision)
    }

    /// Static function schemas cannot express that `initial` becomes invalid
    /// after admission. The host projects the state-valid enum and also repairs
    /// an already-emitted stale control before any sibling operation is judged.
    pub(in crate::chat_runtime) fn normalize_stale_initial(
        &self,
        call: &mut ParsedToolCall,
    ) -> bool {
        if !self.submitted
            || call.name != PLAN_TOOL
            || call
                .arguments
                .get("mode")
                .and_then(serde_json::Value::as_str)
                != Some("initial")
            || self.redundant_stale_initial(call)
        {
            return false;
        }
        call.arguments
            .insert("mode".to_owned(), serde_json::json!("revise"));
        true
    }

    pub(in crate::chat_runtime) fn redundant_stale_initial(&self, call: &ParsedToolCall) -> bool {
        self.submitted
            && call.name == PLAN_TOOL
            && call
                .arguments
                .get("mode")
                .and_then(serde_json::Value::as_str)
                == Some("initial")
            && self.accepted_plan_signature.as_deref() == plan_signature(call).as_deref()
    }

    pub(in crate::chat_runtime) fn redundant_plan_value(&self) -> serde_json::Value {
        let next_stage_id = self
            .primary_ready_step_index()
            .map(|index| self.forecast_steps[index].id.clone());
        serde_json::json!({
            "accepted": true,
            "mode": "unchanged",
            "revision": self.revision,
            "estimatedCalls": self.estimated_calls,
            "workingAllowance": self.working_allowance.saturating_sub(self.operations_used),
            "revisionsRemaining": MAX_PLAN_REVISIONS.saturating_sub(self.revisions_used),
            "toolsRemainAvailable": true,
            "planUnchanged": true,
            "nextStageId": next_stage_id,
            "continuation": "continue_current_stage",
            "guidance": "Plan already accepted and unchanged. Continue the current stage with a fresh task call using retained inputs; do not resubmit this initial plan. Normal permissions and approvals still apply.",
            "learnedFactsBelongInStageResults": true
        })
    }

    /// Recovers from a stale host cursor without granting any effectful
    /// capability. A call matching completed read work reopens that suspended
    /// step; another globally advertised safe read receives a one-result escape
    /// and is then treated as a deviation requiring return or revision.
    pub(in crate::chat_runtime) fn authorize_safe_read_recovery(
        &mut self,
        calls: &[ParsedToolCall],
        all_tools: &[PromptToolDefinition],
    ) -> bool {
        if !self.active()
            || self.forecast_required()
            || calls.is_empty()
            || calls.iter().any(|call| {
                !is_safe_read_tool(&call.name)
                    || !all_tools.iter().any(|tool| tool.name == call.name)
            })
        {
            return false;
        }
        let mut changed = false;
        let mut tool_surface_changed = false;
        for call in calls {
            if self.reopen_completed_read(call) {
                changed = true;
            } else {
                let inserted = self.safe_read_tools.insert(call.name.clone());
                changed |= inserted;
                tool_surface_changed |= inserted;
                self.pending_safe_read_escapes.insert(call.name.clone());
            }
        }
        if tool_surface_changed {
            self.tool_surface_revision = self.tool_surface_revision.saturating_add(1);
        }
        changed
    }

    pub(in crate::chat_runtime) fn validate_calls(
        &mut self,
        calls: &[ParsedToolCall],
    ) -> Result<(), IpcError> {
        let plan_count = calls.iter().filter(|call| call.name == PLAN_TOOL).count();
        let forecast_count = calls
            .iter()
            .filter(|call| call.name == FORECAST_TOOL)
            .count();
        if plan_count > 1
            || (plan_count == 1 && calls.first().is_none_or(|call| call.name != PLAN_TOOL))
        {
            return Err(policy_error(
                "A provisional plan or revision must be the first and only planning call in its tool batch",
            ));
        }
        if self.initial_plan_required(calls) {
            return Err(policy_error(
                "This staged capability was not executed. Submit the semantic plan by itself first; after host acceptance, call the promoted operation in a later response",
            ));
        }
        if plan_count == 1 {
            return Ok(());
        }
        if forecast_count > 1
            || (forecast_count == 1 && calls.first().is_none_or(|call| call.name != FORECAST_TOOL))
        {
            return Err(policy_error(
                "An advisory forecast must be the first and only forecast control in its tool batch",
            ));
        }
        if forecast_count == 1 {
            return Ok(());
        }
        if let Some(error) = self.pre_execution_evidence_deviation(calls) {
            return Err(error);
        }
        if self
            .contract_version
            .is_some_and(|version| version.is_semantic() && !version.uses_advisory_stages())
        {
            self.validate_semantic_step_relationships(calls)?;
        }
        if !self
            .contract_version
            .is_some_and(PlanContractVersion::uses_advisory_stages)
        {
            for call in calls {
                if matches!(
                    call.name.as_str(),
                    "filesystem.write_delimited_table" | "orchestration.transform_artifact"
                ) {
                    self.validate_artifact_evidence_bindings(call)?;
                }
            }
        }
        if !self
            .contract_version
            .is_some_and(PlanContractVersion::uses_advisory_stages)
            && self.submitted
            && self
                .operations_used
                .checked_add(calls.len())
                .is_none_or(|total| total > self.working_allowance)
        {
            if self.outcome_execution_reserve_available
                && !self.outcome_satisfied
                && calls.len() == 1
                && self.is_outcome_bearing_call(&calls[0])
            {
                self.outcome_execution_reserve_available = false;
                return Ok(());
            }
            return Err(policy_error(
                "The provisional plan's adaptive working allowance is exhausted; finish from existing evidence or submit a concise revision of the unfinished work",
            ));
        }
        self.resolve_correction();
        Ok(())
    }

    pub(in crate::chat_runtime) fn validate_semantic_step_relationships(
        &mut self,
        calls: &[ParsedToolCall],
    ) -> Result<(), IpcError> {
        for call in calls {
            match self.operation_relationship(call) {
                OperationStepRelationship::PreferredCurrent
                | OperationStepRelationship::AdaptiveCurrent
                | OperationStepRelationship::ReadyIndependent
                | OperationStepRelationship::OutcomeRecovery
                | OperationStepRelationship::PlanControl => {}
                OperationStepRelationship::FutureStep => {
                    return Err(IpcError::plan_mismatch(format!(
                        "Operation `{}` was not executed because it belongs to later plan work with an unmet dependency. Complete the current semantic step, or revise the dependency graph if the accepted order is no longer valid",
                        call.name
                    )));
                }
                OperationStepRelationship::Unrelated => {
                    return Err(IpcError::scope_correction(format!(
                        "Operation `{}` was not executed because the host cannot relate it to the current semantic step. Use a compatible operation that advances the current objective, or submit the smallest plan revision that explains the changed work",
                        call.name
                    )));
                }
                OperationStepRelationship::NoRemainingStep => {
                    return Err(IpcError::plan_mismatch(format!(
                        "Operation `{}` was not executed because the accepted semantic plan has no unfinished step. Provide the final response, or revise only if a concrete required outcome remains unresolved",
                        call.name
                    )));
                }
            }
            if self
                .contract_version
                .is_some_and(PlanContractVersion::uses_typed_obligations)
            {
                let index = self.matching_ready_step_index(call).ok_or_else(|| {
                    if call.name == "filesystem.collect_evidence" {
                        return self.collector_binding_error(call);
                    }
                    IpcError::scope_correction(format!(
                        "Operation `{}` cannot be bound to a dependency-ready typed semantic step. Use a currently exposed compatible operation, or revise the plan if the required work has materially changed",
                        call.name
                    ))
                })?;
                self.bind_prerequisite_obligations(index)?;
                let outputs = self.forecast_steps[index].output_obligations();
                if !operation_can_address_obligations(call, &outputs) {
                    let ids = outputs
                        .iter()
                        .map(|obligation| obligation.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(IpcError::scope_correction(format!(
                        "Operation `{}` cannot prove every output obligation of step `{}` ({ids}). Use an operation with the required result capability; multiple roles require distinct structured collector items whose ids equal the obligation ids",
                        call.name, self.forecast_steps[index].id
                    )));
                }
            }
        }
        Ok(())
    }

    pub(in crate::chat_runtime) fn collector_binding_error(
        &self,
        call: &ParsedToolCall,
    ) -> IpcError {
        let received_item_ids = call
            .arguments
            .get("items")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| item.get("id").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>();
        let accepted_bindings_by_step = self
            .ready_step_indices()
            .into_iter()
            .map(|index| {
                let step = &self.forecast_steps[index];
                serde_json::json!({
                    "stepId": step.id,
                    "items": step
                        .output_obligations()
                        .into_iter()
                        .filter_map(|obligation| {
                            let operations = collector_operations_for_obligation(obligation);
                            (!operations.is_empty()).then(|| serde_json::json!({
                                "id": obligation.id,
                                "allowedOperations": operations
                            }))
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect::<Vec<_>>();
        IpcError::scope_correction(format!(
            "filesystem.collect_evidence items do not jointly satisfy a dependency-ready semantic step. Replace each item with an exact task-local id and compatible operation advertised by the current tool schema. {}",
            serde_json::json!({
                "receivedItemIds": received_item_ids,
                "acceptedBindingsByStep": accepted_bindings_by_step
            })
        ))
    }

    pub(in crate::chat_runtime) fn bind_prerequisite_obligations(
        &mut self,
        index: usize,
    ) -> Result<(), IpcError> {
        let prerequisite_ids = self.forecast_steps[index]
            .prerequisite_obligations()
            .into_iter()
            .map(|obligation| obligation.id.clone())
            .collect::<Vec<_>>();
        for obligation_id in prerequisite_ids {
            if self.forecast_steps[index]
                .satisfied_obligations
                .contains_key(&obligation_id)
            {
                continue;
            }
            let obligation = self.forecast_steps[index]
                .semantic_obligations
                .iter()
                .find(|obligation| obligation.id == obligation_id)
                .cloned()
                .ok_or_else(IpcError::internal)?;
            let used = self.forecast_steps[index]
                .satisfied_obligations
                .values()
                .map(|receipt| (receipt.call_id.as_str(), receipt.output_key.as_str()))
                .collect::<BTreeSet<_>>();
            let dependency_ids = self.forecast_steps[index].depends_on.clone();
            let receipt = dependency_ids
                .iter()
                .filter_map(|dependency_id| {
                    self.forecast_steps
                        .iter()
                        .find(|step| step.id == *dependency_id && step.complete())
                })
                .flat_map(|step| step.satisfied_obligations.values())
                .find(|receipt| {
                    !used.contains(&(receipt.call_id.as_str(), receipt.output_key.as_str()))
                        && receipt.role_kind == obligation.role_kind
                        && receipt.evidence_class >= obligation.evidence_class
                        && postcondition_satisfies(
                            receipt.postcondition,
                            obligation.postcondition,
                        )
                })
                .cloned()
                .ok_or_else(|| {
                    IpcError::plan_mismatch(format!(
                        "Semantic step `{}` is missing independently grounded obligation `{}` (roleKind={}, evidenceClass={}, postcondition={}). Complete a declared dependency that proves this exact role before the outcome-bearing operation",
                        self.forecast_steps[index].id,
                        obligation.id,
                        obligation.role_kind.as_str(),
                        obligation.evidence_class.as_str(),
                        obligation.postcondition.as_str()
                    ))
                })?;
            self.forecast_steps[index]
                .satisfied_obligations
                .insert(obligation_id, receipt);
        }
        Ok(())
    }

    pub(in crate::chat_runtime) fn operation_relationship(
        &self,
        call: &ParsedToolCall,
    ) -> OperationStepRelationship {
        if call.name == PLAN_TOOL {
            return OperationStepRelationship::PlanControl;
        }
        let Some(current) = self.primary_ready_step_index() else {
            return if self.requires_outcome_recovery() && self.is_outcome_bearing_call(call) {
                OperationStepRelationship::OutcomeRecovery
            } else {
                OperationStepRelationship::NoRemainingStep
            };
        };
        let step = &self.forecast_steps[current];
        if step
            .calls
            .iter()
            .any(|forecast| !forecast.completed && forecast.matches(call))
        {
            return OperationStepRelationship::PreferredCurrent;
        }
        if self.ready_step_indices().into_iter().any(|index| {
            index != current
                && self.forecast_steps[index]
                    .calls
                    .iter()
                    .any(|forecast| !forecast.completed && forecast.matches(call))
        }) {
            return OperationStepRelationship::ReadyIndependent;
        }
        if step.matches_semantic_tool(call) {
            return OperationStepRelationship::AdaptiveCurrent;
        }
        if self
            .ready_step_indices()
            .into_iter()
            .any(|index| index != current && self.forecast_steps[index].matches_semantic_tool(call))
        {
            return OperationStepRelationship::ReadyIndependent;
        }
        if self
            .forecast_steps
            .iter()
            .enumerate()
            .any(|(index, later)| {
                later.unfinished()
                    && !self.step_dependencies_complete(index)
                    && later.matches_operation(call)
            })
        {
            OperationStepRelationship::FutureStep
        } else {
            OperationStepRelationship::Unrelated
        }
    }

    #[cfg(any(debug_assertions, test))]
    pub(in crate::chat_runtime) fn admission_records(
        &self,
        calls: &[ParsedToolCall],
        exchanges: &[Result<ChatToolExchange, IpcError>],
    ) -> Vec<OperationAdmissionRecord> {
        self.admission_records_with_stage_contract(calls, exchanges, None)
    }

    pub(in crate::chat_runtime) fn admission_records_with_stage_contract(
        &self,
        calls: &[ParsedToolCall],
        exchanges: &[Result<ChatToolExchange, IpcError>],
        stage_contract: Option<&BoundedStageContract>,
    ) -> Vec<OperationAdmissionRecord> {
        calls
            .iter()
            .zip(exchanges)
            .map(|(call, exchange)| {
                let permission = permission_outcome(exchange);
                OperationAdmissionRecord {
                    call_id: call.call_id.clone(),
                    permission,
                    relationship: self.operation_relationship(call),
                    progress: self.projected_progress_contribution(
                        call,
                        exchange,
                        permission,
                        stage_contract,
                    ),
                }
            })
            .collect()
    }

    pub(in crate::chat_runtime) fn projected_progress_contribution(
        &self,
        call: &ParsedToolCall,
        exchange: &Result<ChatToolExchange, IpcError>,
        permission: OperationPermissionOutcome,
        stage_contract: Option<&BoundedStageContract>,
    ) -> OperationProgressContribution {
        let relationship = self.operation_relationship(call);
        if permission != OperationPermissionOutcome::Admitted || !relationship.can_advance_current()
        {
            return OperationProgressContribution::None;
        }
        let Ok(exchange) = exchange else {
            return OperationProgressContribution::None;
        };
        if !matches!(exchange.result.outcome, ToolOutcome::Completed { .. }) {
            return OperationProgressContribution::Blocked;
        }
        if stage_contract.is_some_and(|contract| {
            contract.matches(call)
                && !stage_postcondition_satisfied(contract.postcondition, exchange)
        }) {
            return OperationProgressContribution::Blocked;
        }
        let provisions = self.matching_forecast_provisions(call);
        let transport_sufficient = exchange_satisfies_forecast(call, exchange, &provisions);
        let semantic_sufficient = self
            .matching_ready_step_index(call)
            .map(|index| &self.forecast_steps[index])
            .is_none_or(|step| {
                step.semantic_evidence_satisfied_by(call)
                    && (step.action_class != Some(PlanActionClass::WorkspaceWrite)
                        || !self.expects_file_mutation()
                        || self.is_outcome_bearing_call(call))
            });
        if transport_sufficient && semantic_sufficient {
            OperationProgressContribution::Advanced
        } else {
            OperationProgressContribution::Refinement
        }
    }

    pub(in crate::chat_runtime) fn validate_safe_current_reads(
        &mut self,
        calls: &[ParsedToolCall],
    ) -> Result<(), IpcError> {
        if calls.is_empty() || calls.iter().any(|call| !is_safe_read_tool(&call.name)) {
            return Err(policy_error(
                "Only known non-effecting operations can survive a rejected planning control",
            ));
        }
        self.validate_calls(calls)
    }

    /// Replaces model-authored artifact provenance with the trusted
    /// evidence receipts already established by the accepted plan. Opaque host
    /// identifiers are not a model transport responsibility. Every completed
    /// receipt is retained because a role may legitimately span several files.
    pub(in crate::chat_runtime) fn bind_artifact_evidence(&self, calls: &mut [ParsedToolCall]) {
        for call in calls.iter_mut().filter(|call| {
            matches!(
                call.name.as_str(),
                "filesystem.write_delimited_table" | "orchestration.transform_artifact"
            )
        }) {
            call.arguments.remove("evidence");
            if self
                .contract_version
                .is_some_and(PlanContractVersion::is_semantic)
                && let Some(index) = self.matching_ready_step_index(call)
                && let Some((source, convention)) = self.semantic_artifact_receipts(index)
            {
                call.arguments.insert(
                    "evidence".to_owned(),
                    serde_json::json!([
                        {"callId": source, "role": "source"},
                        {"callId": convention, "role": "convention"}
                    ]),
                );
                continue;
            }
            let mut bindings = Vec::new();
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
                let ids = self
                    .completed_evidence
                    .iter()
                    .filter(|evidence| {
                        evidence.provision.role == role && evidence.provision.class >= minimum
                    })
                    .map(|evidence| evidence.call_id.as_str())
                    .collect::<BTreeSet<_>>();
                for call_id in ids {
                    bindings.push(serde_json::json!({
                        "callId": call_id,
                        "role": role_name,
                    }));
                }
            }
            if bindings.iter().any(|binding| binding["role"] == "source")
                && bindings
                    .iter()
                    .any(|binding| binding["role"] == "convention")
            {
                call.arguments
                    .insert("evidence".to_owned(), serde_json::Value::Array(bindings));
            }
        }
    }

    pub(in crate::chat_runtime) fn pre_execution_evidence_deviation(
        &self,
        calls: &[ParsedToolCall],
    ) -> Option<IpcError> {
        if !self.submitted
            || self
                .contract_version
                .is_some_and(PlanContractVersion::is_semantic)
        {
            return None;
        }
        let divergent = calls.iter().find(|call| {
            is_evidence_read_tool(&call.name)
                && !self.pending_safe_read_escapes.contains(&call.name)
                && self.diagnostic_match_unconditional(call) != ForecastDiagnosticMatch::Matched
        })?;
        let expected = self.current_step_unfinished_tools();
        Some(policy_error(&format!(
            "The evidence operation `{}` is outside the accepted unfinished plan and was not executed. Continue with the current forecasted operation(s): {}. If new evidence makes a different read necessary, submit mode=revise first and identify the concrete unresolved evidence gap",
            divergent.name,
            if expected.is_empty() {
                "none".to_owned()
            } else {
                expected.join(", ")
            }
        )))
    }

    #[expect(
        clippy::too_many_lines,
        reason = "plan admission keeps legacy and advertised contract migration in one auditable path"
    )]
    pub(in crate::chat_runtime) fn accept(
        &mut self,
        call: &ParsedToolCall,
        all_tools: &[PromptToolDefinition],
        remaining_calls: usize,
    ) -> Result<serde_json::Value, IpcError> {
        let (contract_version, outcome) = plan_contract_metadata(call)?;
        let mode = string(&call.arguments, "mode")?;
        match mode {
            "initial" if !self.submitted => {}
            "revise" if self.submitted && self.revisions_used < MAX_PLAN_REVISIONS => {}
            "initial" => return Err(policy_error("The initial plan was already submitted")),
            "revise" => {
                return Err(policy_error(
                    "The provisional plan reached its revision limit",
                ));
            }
            _ => return Err(invalid_arguments()),
        }
        if mode == "revise"
            && contract_version.is_semantic()
            && self.accepted_plan_signature.as_deref() == plan_signature(call).as_deref()
        {
            self.resolve_correction();
            return Ok(self.redundant_plan_value());
        }
        if mode == "revise" && self.contract_version != Some(contract_version) {
            return Err(policy_error(
                "A plan revision must preserve the accepted contractVersion",
            ));
        }

        if mode == "revise"
            && contract_version == PlanContractVersion::LegacyForecastV1
            && self.outcome != Some(outcome)
        {
            return Err(policy_error(
                "A revision must preserve the accepted plan's typed outcome",
            ));
        }
        let (forecast, semantic_contract) = match contract_version {
            PlanContractVersion::LegacyForecastV1 => (parse_steps(call, all_tools)?, None),
            PlanContractVersion::SemanticExecutionV1
            | PlanContractVersion::SemanticExecutionV2
            | PlanContractVersion::SemanticExecutionV3
            | PlanContractVersion::SemanticExecutionV4
            | PlanContractVersion::SemanticExecutionV5
            | PlanContractVersion::SemanticExecutionV6 => {
                let (forecast, contract) = parse_semantic_steps(call, all_tools, contract_version)?;
                (forecast, Some(contract))
            }
        };
        if mode == "revise"
            && contract_version.is_semantic()
            && self.outcome == Some(outcome)
            && self.semantic_contract.as_ref().is_some_and(|old| {
                semantic_contract.as_ref().is_some_and(|new| {
                    if contract_version.uses_advisory_stages() {
                        old.same_executable_plan_as(new)
                    } else {
                        old.same_binding_as(new)
                    }
                })
            })
        {
            self.resolve_correction();
            return Ok(self.redundant_plan_value());
        }
        if mode == "revise" && self.final_response_ready() {
            return Err(policy_error(
                "The accepted plan is complete; provide the final response instead of revising finished work",
            ));
        }
        let mut diagnostics = forecast.diagnostics.clone();
        if !contract_version.uses_advisory_stages()
            && let Err(error) = forecast.validate_proportional_evidence_route()
        {
            diagnostics.push(error.message);
        }
        let estimated_calls = forecast.estimated_calls;
        if let Err(error) = validate_forecast_outcome(&forecast, outcome, &self.completed_evidence)
        {
            diagnostics.push(error.message);
        }
        if !diagnostics.is_empty() {
            return Err(policy_error(&format!(
                "The provisional plan has {} independently detectable contract issue(s): {}",
                diagnostics.len(),
                diagnostics.join(" | ")
            )));
        }
        if estimated_calls > remaining_calls {
            return Err(policy_error(
                "The expected operations exceed the remaining hard safety ceiling",
            ));
        }
        let adaptive_allowance = estimated_calls
            .saturating_mul(2)
            .max(estimated_calls.saturating_add(PLAN_RESERVE_CALLS))
            .min(remaining_calls);

        let outcome_binding_changed =
            self.semantic_outcome_binding_changed(outcome, semantic_contract.as_ref());
        let admitted = AdmittedPlan {
            contract_version,
            outcome,
            forecast,
            semantic_contract,
            adaptive_allowance,
            outcome_binding_changed,
            signature: plan_signature(call),
        };
        Ok(self.install_admitted_plan(mode, admitted))
    }

    pub(super) fn semantic_outcome_binding_changed(
        &self,
        outcome: PlannedOutcome,
        new_contract: Option<&SemanticPlanContract>,
    ) -> bool {
        self.outcome.is_some_and(|old| old != outcome)
            || self.semantic_contract.as_ref().is_some_and(|old| {
                new_contract.is_some_and(|new| {
                    old.target != new.target
                        || old.constraints != new.constraints
                        || old.success_criteria != new.success_criteria
                        || old.assumptions != new.assumptions
                })
            })
    }

    pub(super) fn install_admitted_plan(
        &mut self,
        mode: &str,
        mut admitted: AdmittedPlan,
    ) -> serde_json::Value {
        let retained_completed = if mode == "revise" && admitted.contract_version.is_semantic() {
            retain_semantic_progress(
                &mut admitted.forecast.steps,
                admitted.semantic_contract.as_ref(),
                self.semantic_contract.as_ref(),
                &self.forecast_steps,
                admitted.outcome_binding_changed,
                admitted.contract_version.uses_advisory_stages(),
            )
        } else {
            0
        };
        if mode == "revise" && admitted.contract_version.uses_advisory_stages() {
            let rebound = admitted
                .forecast
                .steps
                .iter()
                .flat_map(|step| step.semantic_completed_call_ids.iter().cloned())
                .collect::<BTreeSet<_>>();
            let orphaned = self
                .forecast_steps
                .iter()
                .flat_map(|step| step.semantic_completed_call_ids.iter().cloned())
                .filter(|call_id| !rebound.contains(call_id))
                .collect::<Vec<_>>();
            retain_recent_unique(
                &mut self.retained_advisory_call_ids,
                orphaned,
                MAX_RETAINED_ADVISORY_RESULTS,
            );
        }
        if mode == "revise" {
            self.revisions_used = self.revisions_used.saturating_add(1);
            let superseded_completed = self
                .forecast_steps
                .iter()
                .filter(|step| step.complete())
                .count()
                .saturating_sub(retained_completed);
            self.completed_before_revision = self
                .completed_before_revision
                .saturating_add(superseded_completed);
        }
        self.resolve_correction();
        self.pending_deviation = None;
        self.submitted = true;
        self.revision = self.revision.saturating_add(1);
        self.estimated_calls = admitted.forecast.estimated_calls;
        self.working_allowance = self
            .operations_used
            .saturating_add(admitted.adaptive_allowance);
        self.outcome = Some(admitted.outcome);
        if admitted.outcome_binding_changed {
            self.outcome_satisfied = false;
            self.verified_outcome_call_ids.clear();
            self.outcome_execution_reserve_available = matches!(
                admitted.outcome,
                PlannedOutcome::CreateFile | PlannedOutcome::ModifyFile
            );
        }
        self.forecast_steps = admitted.forecast.steps;
        self.contract_version = Some(admitted.contract_version);
        self.semantic_contract = admitted.semantic_contract;
        if admitted.contract_version.requires_execution_forecast() {
            self.execution_forecast = None;
        }
        self.accepted_plan_signature = admitted.signature;
        self.evidence_refinements_used = 0;
        self.refinement_step_id = (!self.unresolved_evidence_call_ids.is_empty())
            .then(|| self.primary_ready_step_index())
            .flatten()
            .map(|index| self.forecast_steps[index].id.clone());
        self.safe_read_tools.clear();
        self.pending_safe_read_escapes.clear();
        if mode == "initial" {
            self.outcome_execution_reserve_available = matches!(
                admitted.outcome,
                PlannedOutcome::CreateFile | PlannedOutcome::ModifyFile
            );
        }

        serde_json::json!({
            "accepted": true,
            "contractVersion": admitted.contract_version.as_str(),
            "mode": mode,
            "revision": self.revision,
            "estimatedCalls": self.estimated_calls,
            "workingAllowance": admitted.adaptive_allowance,
            "revisionsRemaining": MAX_PLAN_REVISIONS.saturating_sub(self.revisions_used),
            "toolsRemainAvailable": true
        })
    }
}
