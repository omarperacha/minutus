//! Tool-surface projection and dependency-ready plan routing.

#[cfg(debug_assertions)]
use super::ForecastDiagnosticMatch;

use super::{
    BoundedStageContract, COMPLETE_STAGE_TOOL, EXECUTION_FORECAST_CONTRACT_VERSION, FORECAST_TOOL,
    ForecastCall, ForecastStep, IpcError, LEGACY_EXECUTION_FORECAST_CONTRACT_VERSION,
    MAX_PLAN_REVISIONS, ObligationReceipt, PLAN_TOOL, ParsedToolCall, PlanActionClass,
    PlanContractVersion, PlanEvidenceClass, PlanEvidenceRoleKind, PlanPolicy, PromptToolDefinition,
    ROLE_AWARE_EXECUTION_FORECAST_CONTRACT_VERSION, SEARCH_TOOLS_TOOL, ToolCategory,
    collector_operations_for_obligation, dependency_output_evidence_classes, invalid_arguments,
    operation_can_address_obligations, operation_name_can_address_obligations,
    parse_execution_forecast, policy_error, postcondition_satisfies, project_collector_item_schema,
    project_forecast_mode, project_plan_mode, project_tool_definitions, stage_postcondition,
    string, tool_category,
};
use std::collections::{BTreeMap, BTreeSet};

mod semantic;

impl PlanPolicy {
    /// Exposes only the unfinished accepted step. A blocked step or exact
    /// validation correction additionally exposes plan revision as the bounded
    /// escape hatch; ordinary execution cannot silently expand scope.
    pub(in crate::chat_runtime) fn tools(
        &self,
        all: &[PromptToolDefinition],
    ) -> Vec<PromptToolDefinition> {
        if self.final_response_ready() {
            return Vec::new();
        }
        if !self.active() {
            return all
                .iter()
                .filter(|tool| {
                    matches!(
                        tool.name.as_str(),
                        "web.search"
                            | "web.retrieve"
                            | "filesystem.list"
                            | "filesystem.find_paths"
                            | "x402.request"
                            | "wallet.request_transfer"
                            | PLAN_TOOL
                    )
                })
                .cloned()
                .map(|tool| {
                    if tool.name == "x402.request" {
                        return crate::chat_tools::discovery_tool_definition();
                    }
                    if tool.name == PLAN_TOOL {
                        project_plan_mode(tool, "initial")
                    } else {
                        tool
                    }
                })
                .collect();
        }
        if self.forecast_required() {
            return all
                .iter()
                .filter(|tool| tool.name == FORECAST_TOOL)
                .cloned()
                .map(|tool| project_forecast_mode(tool, "initial"))
                .collect();
        }
        if self
            .contract_version
            .is_some_and(PlanContractVersion::uses_advisory_stages)
        {
            return self.advisory_stage_tools(all);
        }
        if self
            .contract_version
            .is_some_and(PlanContractVersion::is_semantic)
        {
            if self
                .contract_version
                .is_some_and(PlanContractVersion::uses_host_routing)
            {
                return all
                    .iter()
                    .filter(|tool| self.host_routed_tool_available(&tool.name))
                    .cloned()
                    .map(|tool| self.project_host_routed_tool(tool))
                    .map(|tool| project_plan_mode(tool, "revise"))
                    .collect();
            }
            let projected_categories = self.projected_forecast_categories();
            let projected =
                if self.contract_version == Some(PlanContractVersion::SemanticExecutionV1) {
                    all.to_vec()
                } else {
                    project_tool_definitions(all, &projected_categories, &self.safe_read_tools)
                };
            return projected
                .into_iter()
                .filter(|tool| self.v3_tool_available(tool.name.as_str()))
                .map(|tool| project_forecast_mode(project_plan_mode(tool, "revise"), "revise"))
                .collect();
        }
        let unfinished = self
            .current_step_unfinished_tools()
            .into_iter()
            .collect::<BTreeSet<_>>();
        all.iter()
            .filter(|tool| {
                unfinished.contains(tool.name.as_str())
                    || self.safe_read_tools.contains(&tool.name)
                    || (tool.name == PLAN_TOOL && self.revision_available())
            })
            .cloned()
            .map(|tool| project_plan_mode(tool, "revise"))
            .collect()
    }

    pub(in crate::chat_runtime) const fn active(&self) -> bool {
        self.submitted
    }

    pub(in crate::chat_runtime) fn load_task_tools(
        &mut self,
        names: impl IntoIterator<Item = String>,
    ) {
        let before = self.loaded_task_tool_names.len();
        self.loaded_task_tool_names.extend(names);
        if self.loaded_task_tool_names.len() != before {
            self.tool_surface_revision = self.tool_surface_revision.saturating_add(1);
        }
    }

    pub(in crate::chat_runtime) const fn tool_surface_revision(&self) -> usize {
        self.tool_surface_revision
    }

    pub(in crate::chat_runtime) fn forecast_required(&self) -> bool {
        self.contract_version
            .is_some_and(PlanContractVersion::requires_execution_forecast)
            && self.execution_forecast.is_none()
    }

    pub(in crate::chat_runtime) fn accepts_execution_forecast(&self) -> bool {
        self.contract_version
            .is_some_and(PlanContractVersion::requires_execution_forecast)
    }

    pub(in crate::chat_runtime) fn requires_plan_only_round(&self) -> bool {
        self.contract_version
            .is_some_and(PlanContractVersion::requires_plan_only_round)
    }

    pub(in crate::chat_runtime) fn uses_advisory_stages(&self) -> bool {
        self.contract_version
            .is_some_and(PlanContractVersion::uses_advisory_stages)
    }

    pub(in crate::chat_runtime) fn current_stage_id(&self) -> Option<&str> {
        self.primary_ready_step_index()
            .map(|index| self.forecast_steps[index].id.as_str())
    }

    pub(in crate::chat_runtime) const fn forecast_revision(&self) -> usize {
        self.forecast_revisions
    }

    pub(in crate::chat_runtime) fn projected_forecast_categories(&self) -> BTreeSet<ToolCategory> {
        let Some(forecast) = &self.execution_forecast else {
            return BTreeSet::new();
        };
        let mut categories = BTreeSet::new();
        for index in self.ready_step_indices() {
            let step_id = &self.forecast_steps[index].id;
            if let Some(category) = forecast
                .entries
                .iter()
                .find(|entry| entry.step_id == *step_id)
                .and_then(|entry| ToolCategory::parse(&entry.category))
            {
                categories.insert(category);
                if categories.len() == 2 {
                    break;
                }
            }
        }
        categories
    }

    pub(in crate::chat_runtime) fn forecast_scope(&self) -> String {
        format!("plan:{}:forecast", self.revision)
    }

    pub(in crate::chat_runtime) fn accept_forecast(
        &mut self,
        call: &ParsedToolCall,
        all_tools: &[PromptToolDefinition],
    ) -> Result<serde_json::Value, IpcError> {
        if !self.accepts_execution_forecast() {
            return Err(policy_error(
                "An execution forecast is accepted only after a legacy semantic_execution_v2 through v4 plan",
            ));
        }
        let version = string(&call.arguments, "contractVersion")?;
        let expected_version = match self.contract_version {
            Some(PlanContractVersion::SemanticExecutionV2) => {
                LEGACY_EXECUTION_FORECAST_CONTRACT_VERSION
            }
            Some(PlanContractVersion::SemanticExecutionV3) => {
                ROLE_AWARE_EXECUTION_FORECAST_CONTRACT_VERSION
            }
            Some(PlanContractVersion::SemanticExecutionV4) => EXECUTION_FORECAST_CONTRACT_VERSION,
            _ => return Err(policy_error("The accepted plan has no forecast contract")),
        };
        if version != expected_version {
            return Err(policy_error(&format!(
                "contractVersion must be {expected_version}"
            )));
        }
        let mode = string(&call.arguments, "mode")?;
        match (mode, self.execution_forecast.is_some()) {
            ("initial", false) | ("revise", true) => {}
            ("initial", true) => {
                return Err(policy_error(
                    "The initial execution forecast was already submitted",
                ));
            }
            ("revise", false) => {
                return Err(policy_error(
                    "Submit the initial execution forecast before revising it",
                ));
            }
            _ => return Err(invalid_arguments()),
        }
        let contract = self.semantic_contract.as_ref().ok_or_else(|| {
            policy_error("The accepted semantic plan is unavailable for forecast validation")
        })?;
        let contract_version = self
            .contract_version
            .ok_or_else(|| policy_error("The accepted semantic plan version is unavailable"))?;
        let forecast = parse_execution_forecast(call, contract, contract_version, all_tools)?;
        for entry in &forecast.entries {
            if let Some(step) = self
                .forecast_steps
                .iter_mut()
                .find(|step| step.id == entry.step_id)
            {
                step.calls = entry
                    .operations
                    .iter()
                    .map(|operation| ForecastCall {
                        tool: operation.clone(),
                        purpose: entry.purpose.clone(),
                        scope: None,
                        provides_evidence: Vec::new(),
                        completed: false,
                        completed_call_id: None,
                    })
                    .collect();
            }
        }
        self.execution_forecast = Some(forecast.clone());
        self.forecast_revisions = self.forecast_revisions.saturating_add(1);
        Ok(serde_json::json!({
            "accepted": true,
            "contractVersion": expected_version,
            "mode": mode,
            "revision": self.forecast_revisions,
            "entryCount": forecast.entries.len(),
            "advisory": true,
            "permissionGranted": false,
            "progressCredited": false
        }))
    }

    #[cfg(debug_assertions)]
    pub(in crate::chat_runtime) fn forecast_diagnostic(
        &self,
        call: &ParsedToolCall,
    ) -> (&'static str, Option<&str>) {
        let Some(forecast) = &self.execution_forecast else {
            return ("unforecasted", None);
        };
        let ready = self.ready_step_indices();
        let matching = forecast.entries.iter().find(|entry| {
            entry
                .operations
                .iter()
                .any(|operation| operation == &call.name)
                && ready
                    .iter()
                    .any(|index| self.forecast_steps[*index].id == entry.step_id)
        });
        matching.map_or(("divergent", None), |entry| {
            ("matched", Some(entry.category.as_str()))
        })
    }

    #[cfg(debug_assertions)]
    pub(in crate::chat_runtime) fn forecast_telemetry(&self) -> serde_json::Value {
        serde_json::json!({
            "revision": self.forecast_revisions,
            "entries": self.execution_forecast.as_ref().map_or_else(Vec::new, |forecast| {
                forecast.entries.iter().enumerate().map(|(index, entry)| serde_json::json!({
                    "entryOrdinal": index.saturating_add(1),
                    "category": entry.category,
                    "operations": entry.operations,
                })).collect::<Vec<_>>()
            })
        })
    }

    pub(in crate::chat_runtime) const fn revision(&self) -> usize {
        self.revision
    }

    pub(in crate::chat_runtime) fn current_step_unfinished_tools(&self) -> Vec<&str> {
        let mut seen = BTreeSet::new();
        let mut tools = Vec::new();
        for index in self.ready_step_indices() {
            for call in &self.forecast_steps[index].calls {
                let tool = call.tool.as_str();
                if !call.completed && seen.insert(tool) {
                    tools.push(tool);
                }
            }
        }
        tools
    }

    pub(super) fn advisory_completion_stage(&self) -> Option<&ForecastStep> {
        self.primary_ready_step_index()
            .map(|index| &self.forecast_steps[index])
            .filter(|step| !step.semantic_completed_call_ids.is_empty())
    }

    pub(in crate::chat_runtime) fn stage_completion_decision_due(&self) -> bool {
        self.advisory_completion_stage()
            .is_some_and(|step| step.semantic_completed_call_ids.len() >= 2)
    }

    pub(in crate::chat_runtime) fn current_step_relevant_tools<'a>(
        &self,
        all: &'a [PromptToolDefinition],
    ) -> Vec<&'a str> {
        if !self
            .contract_version
            .is_some_and(PlanContractVersion::is_semantic)
        {
            let unfinished = self
                .current_step_unfinished_tools()
                .into_iter()
                .collect::<BTreeSet<_>>();
            return all
                .iter()
                .filter(|tool| unfinished.contains(tool.name.as_str()))
                .map(|tool| tool.name.as_str())
                .collect();
        }
        if self
            .contract_version
            .is_some_and(PlanContractVersion::uses_advisory_stages)
        {
            if self.primary_ready_step_index().is_none() && self.requires_outcome_recovery() {
                return all
                    .iter()
                    .filter(|tool| {
                        self.is_outcome_bearing_tool(&tool.name) || tool.name == PLAN_TOOL
                    })
                    .map(|tool| tool.name.as_str())
                    .collect();
            }
            let suggested = self
                .current_step_unfinished_tools()
                .into_iter()
                .collect::<BTreeSet<_>>();
            let ready = self.ready_step_indices();
            return all
                .iter()
                .filter(|tool| {
                    suggested.contains(tool.name.as_str())
                        || ready.iter().any(|index| {
                            self.forecast_steps[*index]
                                .action_class
                                .is_some_and(|class| class.matches_tool(&tool.name))
                        })
                        || tool.name == PLAN_TOOL
                        || (tool.name == COMPLETE_STAGE_TOOL
                            && self.primary_ready_step_index().is_some())
                })
                .map(|tool| tool.name.as_str())
                .collect();
        }
        if self
            .contract_version
            .is_some_and(PlanContractVersion::uses_host_routing)
        {
            return all
                .iter()
                .filter(|tool| self.host_routed_tool_available(&tool.name))
                .map(|tool| tool.name.as_str())
                .collect();
        }
        let ready = self.ready_step_indices();
        let projected_categories = self.projected_forecast_categories();
        all.iter()
            .filter(|tool| {
                if self.contract_version.is_some_and(|version| {
                    matches!(
                        version,
                        PlanContractVersion::SemanticExecutionV2
                            | PlanContractVersion::SemanticExecutionV3
                            | PlanContractVersion::SemanticExecutionV4
                    )
                }) && !tool_category(&tool.name)
                    .is_some_and(|category| projected_categories.contains(&category))
                {
                    return false;
                }
                ready.iter().any(|index| {
                    let step = &self.forecast_steps[*index];
                    self.v3_step_tool_ready(*index, tool.name.as_str())
                        && (step.calls.iter().any(|call| call.tool == tool.name)
                            || step
                                .action_class
                                .is_some_and(|class| class.matches_tool(&tool.name)))
                })
            })
            .map(|tool| tool.name.as_str())
            .collect()
    }

    pub(in crate::chat_runtime) fn post_completion_relevant_tools<'a>(
        &self,
        all: &'a [PromptToolDefinition],
    ) -> Vec<&'a str> {
        if !self
            .contract_version
            .is_some_and(PlanContractVersion::uses_advisory_stages)
            || self.primary_ready_step_index().is_none()
        {
            return Vec::new();
        }
        let Some(index) = self.primary_ready_step_index() else {
            return Vec::new();
        };
        let mut preview = self.clone();
        preview.forecast_steps[index].semantic_complete = true;
        preview
            .current_step_relevant_tools(all)
            .into_iter()
            .filter(|tool| *tool != PLAN_TOOL && *tool != COMPLETE_STAGE_TOOL)
            .collect()
    }
}
