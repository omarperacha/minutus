#[cfg(debug_assertions)]
use super::ForecastDiagnosticMatch;

use super::{
    BTreeMap, BTreeSet, BoundedStageContract, COMPLETE_STAGE_TOOL, FORECAST_TOOL, ForecastStep,
    MAX_PLAN_REVISIONS, ObligationReceipt, PLAN_TOOL, ParsedToolCall, PlanActionClass,
    PlanContractVersion, PlanEvidenceClass, PlanEvidenceRoleKind, PlanPolicy, PromptToolDefinition,
    SEARCH_TOOLS_TOOL, collector_operations_for_obligation, dependency_output_evidence_classes,
    operation_can_address_obligations, operation_name_can_address_obligations,
    postcondition_satisfies, project_collector_item_schema, project_plan_mode, stage_postcondition,
};

impl PlanPolicy {
    pub(in crate::chat_runtime) fn v3_tool_available(&self, operation: &str) -> bool {
        if !self
            .contract_version
            .is_some_and(PlanContractVersion::uses_typed_obligations)
            || operation == PLAN_TOOL
            || operation == FORECAST_TOOL
        {
            return true;
        }
        let is_outcome_operation = PlanActionClass::WorkspaceWrite.matches_tool(operation)
            || PlanActionClass::ExternalWrite.matches_tool(operation);
        !is_outcome_operation
            || self
                .ready_step_indices()
                .into_iter()
                .any(|index| self.v3_step_tool_ready(index, operation))
    }

    pub(in crate::chat_runtime) fn host_routed_tool_available(&self, operation: &str) -> bool {
        if operation == PLAN_TOOL {
            return self.revision_available();
        }
        if operation == FORECAST_TOOL {
            return false;
        }
        if self.safe_read_tools.contains(operation) {
            return true;
        }
        self.ready_step_indices().into_iter().any(|index| {
            self.v3_step_tool_ready(index, operation)
                && self.forecast_steps[index]
                    .action_class
                    .is_some_and(|class| class.matches_tool(operation))
                && operation_name_can_address_obligations(
                    operation,
                    &self.forecast_steps[index].output_obligations(),
                )
        })
    }

    pub(in crate::chat_runtime) fn advisory_stage_tools(
        &self,
        all: &[PromptToolDefinition],
    ) -> Vec<PromptToolDefinition> {
        // Keep the plan-wide suggested surface stable after admission. The full
        // catalogue remains reachable through search without paying for every
        // complete schema on every ordinary round.
        let mut suggested = self
            .forecast_steps
            .iter()
            .flat_map(|step| step.calls.iter().map(|call| call.tool.as_str()))
            .collect::<BTreeSet<_>>();
        let companions = suggested
            .iter()
            .copied()
            .chain(self.loaded_task_tool_names.iter().map(String::as_str))
            .flat_map(crate::chat_tools::tool_companions)
            .copied()
            .collect::<Vec<_>>();
        // Only real available definitions below can be promoted. Read-only
        // workspaces cannot gain a writer through this catalogue relationship.
        suggested.extend(companions);
        all.iter()
            .filter(|tool| {
                suggested.contains(tool.name.as_str())
                    || self.loaded_task_tool_names.contains(&tool.name)
                    || self.safe_read_tools.contains(&tool.name)
                    || tool.name == COMPLETE_STAGE_TOOL
                    || tool.name == SEARCH_TOOLS_TOOL
                    || (tool.name == PLAN_TOOL && self.revision_available())
            })
            .cloned()
            .map(|tool| project_plan_mode(tool, "revise"))
            .collect()
    }

    pub(in crate::chat_runtime) fn project_host_routed_tool(
        &self,
        mut tool: PromptToolDefinition,
    ) -> PromptToolDefinition {
        if tool.name != "filesystem.collect_evidence" {
            return tool;
        }
        let accepted_bindings = self
            .ready_step_indices()
            .into_iter()
            .flat_map(|index| {
                self.forecast_steps[index]
                    .output_obligations()
                    .into_iter()
                    .filter_map(|obligation| {
                        let operations = collector_operations_for_obligation(obligation);
                        (!operations.is_empty()).then(|| (obligation.id.clone(), operations))
                    })
            })
            .collect::<Vec<_>>();
        if accepted_bindings.is_empty() {
            return tool;
        }
        let obligation_ids = accepted_bindings
            .iter()
            .map(|(id, _)| id.clone())
            .collect::<BTreeSet<_>>();
        let operations = accepted_bindings
            .iter()
            .flat_map(|(_, operations)| operations.iter().copied())
            .collect::<BTreeSet<_>>();
        if let Some(item_schema) = tool.parameters.pointer_mut("/properties/items/items") {
            project_collector_item_schema(item_schema, &obligation_ids, &operations);
            if let Some(variants) = item_schema
                .get_mut("oneOf")
                .and_then(serde_json::Value::as_array_mut)
            {
                for variant in variants {
                    project_collector_item_schema(variant, &obligation_ids, &operations);
                }
            }
        }
        tool
    }

    pub(in crate::chat_runtime) fn v3_step_tool_ready(
        &self,
        index: usize,
        operation: &str,
    ) -> bool {
        let Some(step) = self.forecast_steps.get(index) else {
            return false;
        };
        if !step.calls.iter().any(|call| call.tool == operation)
            && !step
                .action_class
                .is_some_and(|class| class.matches_tool(operation))
        {
            return false;
        }
        if !self
            .contract_version
            .is_some_and(PlanContractVersion::uses_typed_obligations)
            || !matches!(
                step.action_class,
                Some(PlanActionClass::WorkspaceWrite | PlanActionClass::ExternalWrite)
            )
        {
            return true;
        }
        self.v3_prerequisite_receipts(index).is_some()
    }

    /// Returns a strict stage contract only for one exact, dependency-ready
    /// semantic operation whose structural result the host can verify. Plan
    /// prose and untrusted evidence contents never participate in readiness.
    pub(in crate::chat_runtime) fn bounded_stage_contract(
        &self,
        all: &[PromptToolDefinition],
        workspace_mutation_tools_available: bool,
    ) -> Option<BoundedStageContract> {
        if self
            .contract_version
            .is_some_and(PlanContractVersion::uses_advisory_stages)
        {
            return None;
        }
        if !self
            .contract_version
            .is_some_and(PlanContractVersion::is_semantic)
            || self.correction_pending()
            || self.revision_recovery_pending()
            || self.refinement_active()
            || !self.revision_available()
        {
            return None;
        }
        let index = self.primary_ready_step_index()?;
        let step = &self.forecast_steps[index];
        if step.blocked || !self.step_dependencies_complete(index) || step.calls.len() != 1 {
            return None;
        }
        let operation = step.calls.first()?.tool.clone();
        let action_class = step.action_class?;
        if !action_class.matches_tool(&operation)
            || !all.iter().any(|tool| tool.name == operation)
            || (action_class == PlanActionClass::WorkspaceWrite
                && !workspace_mutation_tools_available)
        {
            return None;
        }
        let postcondition = stage_postcondition(&operation)?;
        let evidence_call_ids = if matches!(
            operation.as_str(),
            "filesystem.write_delimited_table" | "orchestration.transform_artifact"
        ) {
            let (source, convention) = self.semantic_artifact_receipts(index)?;
            [source, convention].into_iter().collect()
        } else {
            self.required_stage_evidence_receipts(index)?
        };
        if evidence_call_ids.is_empty() {
            return None;
        }
        Some(BoundedStageContract {
            operation,
            postcondition,
            evidence_call_ids,
        })
    }

    pub(in crate::chat_runtime) fn required_stage_evidence_receipts(
        &self,
        index: usize,
    ) -> Option<Vec<String>> {
        if self
            .contract_version
            .is_some_and(PlanContractVersion::uses_typed_obligations)
        {
            let mut receipts = self
                .v3_prerequisite_receipts(index)?
                .into_values()
                .map(|receipt| receipt.call_id)
                .collect::<Vec<_>>();
            receipts.sort();
            receipts.dedup();
            return Some(receipts);
        }
        let step = self.forecast_steps.get(index)?;
        let dependencies = step
            .depends_on
            .iter()
            .map(|dependency| {
                self.forecast_steps
                    .iter()
                    .find(|candidate| candidate.id == *dependency)
                    .filter(|candidate| candidate.complete())
            })
            .collect::<Option<Vec<_>>>()?;
        let mut receipts = Vec::new();
        let prerequisite_evidence = if step.action_class == Some(PlanActionClass::Read) {
            &[][..]
        } else {
            step.semantic_required_evidence.as_slice()
        };
        for required in prerequisite_evidence {
            let dependency = dependencies.iter().find(|dependency| {
                dependency_output_evidence_classes(dependency).contains(required)
                    && !dependency.semantic_completed_call_ids.is_empty()
            })?;
            receipts.extend(dependency.semantic_completed_call_ids.iter().cloned());
        }
        if prerequisite_evidence.is_empty() {
            receipts.extend(
                dependencies
                    .iter()
                    .flat_map(|dependency| dependency.semantic_completed_call_ids.iter().cloned()),
            );
        }
        receipts.sort();
        receipts.dedup();
        Some(receipts)
    }

    pub(in crate::chat_runtime) fn semantic_artifact_receipts(
        &self,
        index: usize,
    ) -> Option<(String, String)> {
        if self
            .contract_version
            .is_some_and(PlanContractVersion::uses_typed_obligations)
        {
            let receipts = self.v3_prerequisite_receipts(index)?;
            let source = receipts.iter().find_map(|(id, receipt)| {
                self.forecast_steps[index]
                    .semantic_obligations
                    .iter()
                    .find(|obligation| obligation.id == *id)
                    .filter(|obligation| obligation.role_kind == PlanEvidenceRoleKind::Source)
                    .map(|_| receipt.call_id.clone())
            })?;
            let convention = receipts.iter().find_map(|(id, receipt)| {
                self.forecast_steps[index]
                    .semantic_obligations
                    .iter()
                    .find(|obligation| obligation.id == *id)
                    .filter(|obligation| obligation.role_kind == PlanEvidenceRoleKind::Convention)
                    .map(|_| receipt.call_id.clone())
            })?;
            return Some((source, convention));
        }
        let source = self.semantic_dependency_receipt(index, PlanEvidenceClass::SemanticContent)?;
        let convention =
            self.semantic_dependency_receipt(index, PlanEvidenceClass::FormatStructure)?;
        Some((source, convention))
    }

    pub(in crate::chat_runtime::plan) fn semantic_dependency_receipt(
        &self,
        index: usize,
        minimum: PlanEvidenceClass,
    ) -> Option<String> {
        let step = self.forecast_steps.get(index)?;
        [true, false].into_iter().find_map(|exact| {
            step.depends_on.iter().find_map(|dependency| {
                let dependency = self
                    .forecast_steps
                    .iter()
                    .find(|candidate| candidate.id == *dependency && candidate.complete())?;
                let classes = dependency_output_evidence_classes(dependency);
                classes
                    .iter()
                    .any(|provided| {
                        if exact {
                            *provided == minimum
                        } else {
                            *provided >= minimum
                        }
                    })
                    .then(|| dependency.semantic_completed_call_ids.first().cloned())
                    .flatten()
            })
        })
    }

    pub(in crate::chat_runtime::plan) fn v3_prerequisite_receipts(
        &self,
        index: usize,
    ) -> Option<BTreeMap<String, ObligationReceipt>> {
        let step = self.forecast_steps.get(index)?;
        let mut bound = BTreeMap::new();
        let mut used = BTreeSet::new();
        for obligation in step.prerequisite_obligations() {
            let receipt = step
                .depends_on
                .iter()
                .filter_map(|dependency_id| {
                    self.forecast_steps
                        .iter()
                        .find(|candidate| candidate.id == *dependency_id && candidate.complete())
                })
                .flat_map(|dependency| dependency.satisfied_obligations.values())
                .find(|receipt| {
                    !used.contains(&(receipt.call_id.clone(), receipt.output_key.clone()))
                        && receipt.role_kind == obligation.role_kind
                        && receipt.evidence_class >= obligation.evidence_class
                        && postcondition_satisfies(receipt.postcondition, obligation.postcondition)
                })?
                .clone();
            used.insert((receipt.call_id.clone(), receipt.output_key.clone()));
            bound.insert(obligation.id.clone(), receipt);
        }
        Some(bound)
    }

    pub(in crate::chat_runtime) fn current_step_is_workspace_write(&self) -> bool {
        self.ready_step_indices().into_iter().any(|index| {
            self.forecast_steps[index].action_class == Some(PlanActionClass::WorkspaceWrite)
        })
    }

    pub(in crate::chat_runtime) fn current_step_is_compute(&self) -> bool {
        self.ready_step_indices()
            .into_iter()
            .any(|index| self.forecast_steps[index].action_class == Some(PlanActionClass::Compute))
    }

    pub(in crate::chat_runtime) fn step_dependencies_complete(&self, index: usize) -> bool {
        self.forecast_steps[index]
            .depends_on
            .iter()
            .all(|dependency| {
                self.forecast_steps
                    .iter()
                    .find(|step| step.id == *dependency)
                    .is_some_and(ForecastStep::complete)
            })
    }

    pub(in crate::chat_runtime) fn ready_step_indices(&self) -> Vec<usize> {
        self.forecast_steps
            .iter()
            .enumerate()
            .filter(|(index, step)| step.unfinished() && self.step_dependencies_complete(*index))
            .map(|(index, _)| index)
            .collect()
    }

    pub(in crate::chat_runtime) fn primary_ready_step_index(&self) -> Option<usize> {
        self.ready_step_indices().into_iter().next()
    }

    pub(in crate::chat_runtime) fn matching_ready_step_index(
        &self,
        operation: &ParsedToolCall,
    ) -> Option<usize> {
        let ready = self.ready_step_indices();
        ready
            .iter()
            .copied()
            .find(|index| {
                self.forecast_steps[*index]
                    .calls
                    .iter()
                    .any(|forecast| !forecast.completed && forecast.matches(operation))
            })
            .or_else(|| {
                ready.into_iter().find(|index| {
                    let step = &self.forecast_steps[*index];
                    step.matches_semantic_tool(operation)
                        && operation_can_address_obligations(operation, &step.output_obligations())
                })
            })
    }

    pub(in crate::chat_runtime) fn safe_read_escape_tools(&self) -> Vec<&str> {
        self.safe_read_tools.iter().map(String::as_str).collect()
    }

    pub(in crate::chat_runtime) fn execution_scope_locked(&self) -> bool {
        if self
            .contract_version
            .is_some_and(PlanContractVersion::is_semantic)
        {
            return false;
        }
        let Some(step) = self.forecast_steps.iter().find(|step| step.unfinished()) else {
            return false;
        };
        !step.blocked
            && step.calls.iter().any(|call| !call.completed)
            && step
                .calls
                .iter()
                .filter(|call| !call.completed)
                .all(|call| self.is_outcome_bearing_tool(&call.tool))
    }

    pub(in crate::chat_runtime) fn revision_available(&self) -> bool {
        self.submitted && self.revisions_used < MAX_PLAN_REVISIONS && !self.final_response_ready()
    }

    pub(in crate::chat_runtime) fn final_response_ready(&self) -> bool {
        // A write may advance an advisory cursor, but it must not close tools
        // before the model sees its result and decides whether more deliverables
        // remain. Plain final answers need no extra checkpoint.
        let explicitly_completed = !self.uses_advisory_stages()
            || self
                .forecast_steps
                .iter()
                .all(|step| step.semantic_completion_declared);
        self.submitted
            && self.forecast_complete()
            && explicitly_completed
            && (self.outcome_satisfied || !self.expects_file_mutation())
    }

    pub(in crate::chat_runtime) fn unfinished_call_count(&self) -> usize {
        let legacy = self
            .forecast_steps
            .iter()
            .filter(|step| step.action_class.is_none())
            .flat_map(|step| step.calls.iter())
            .filter(|call| !call.completed)
            .count();
        legacy.saturating_add(
            self.forecast_steps
                .iter()
                .filter(|step| step.action_class.is_some() && step.unfinished())
                .count(),
        )
    }

    pub(in crate::chat_runtime) fn forecast_complete(&self) -> bool {
        self.submitted && self.forecast_steps.iter().all(ForecastStep::complete)
    }

    pub(in crate::chat_runtime) const fn deviation_count(&self) -> usize {
        self.revisions_used
            .saturating_add(self.unforecasted_operations)
    }

    #[cfg(debug_assertions)]
    pub(in crate::chat_runtime) fn diagnostic_match(
        &self,
        call: &ParsedToolCall,
    ) -> (usize, Option<usize>, ForecastDiagnosticMatch) {
        if call.name == PLAN_TOOL {
            return (self.revision, None, ForecastDiagnosticMatch::Matched);
        }
        let Some(current) = self.primary_ready_step_index() else {
            return (self.revision, None, ForecastDiagnosticMatch::Unforecasted);
        };
        if self.forecast_steps[current]
            .calls
            .iter()
            .any(|forecast| !forecast.completed && forecast.matches(call))
            || self.forecast_steps[current].matches_semantic_tool(call)
        {
            return (
                self.revision,
                Some(current.saturating_add(1)),
                ForecastDiagnosticMatch::Matched,
            );
        }
        if let Some(independent) = self
            .ready_step_indices()
            .into_iter()
            .find(|index| *index != current && self.forecast_steps[*index].matches_operation(call))
        {
            return (
                self.revision,
                Some(independent.saturating_add(1)),
                ForecastDiagnosticMatch::Matched,
            );
        }
        let later = self
            .forecast_steps
            .iter()
            .enumerate()
            .find(|(index, step)| *index != current && step.matches_operation(call))
            .map(|(index, _)| index.saturating_add(1));
        if let Some(step) = later {
            (
                self.revision,
                Some(step),
                ForecastDiagnosticMatch::OutOfOrder,
            )
        } else {
            (
                self.revision,
                Some(current.saturating_add(1)),
                ForecastDiagnosticMatch::Unforecasted,
            )
        }
    }
}
