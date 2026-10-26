use super::{
    AssetId, BTreeSet, ExecutionProfileDecision, ExecutionWorkKind, GenerationBinding,
    ModelAdapterKind, PromptRenderRequest, PromptToolDefinition, ReasoningStrength,
};

mod history;

#[cfg(test)]
mod plan_pending_tests;
#[cfg(test)]
mod tests;

pub(super) struct WorkingPromptAdapterContext<'a> {
    pub(super) current_date: &'a str,
    pub(super) tools: Vec<PromptToolDefinition>,
    pub(super) adapter: ModelAdapterKind,
    pub(super) reasoning_strength: ReasoningStrength,
}

/// Adapter-neutral prompt prepared and admitted for one generation.
pub enum PreparedChatPrompt {
    /// Fully rendered Glimmer/ATEM prompt for raw Ollama generation.
    Local {
        /// Exact application-rendered ATEM prompt.
        rendered: String,
        /// Structured definitions supplied separately to Ollama's ATEM parser.
        tools: Vec<PromptToolDefinition>,
    },
    /// Native structured Qwen request rendered only after verified assets are resolved.
    Qwen(PromptRenderRequest),
}

/// Content-free timings for the exact provider prompt admission boundary.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PromptPreparationTiming {
    provider_prompt_render_ms: u64,
    exact_tokenization_ms: u64,
}

impl PromptPreparationTiming {
    pub(crate) const fn new(provider_prompt_render_ms: u64, exact_tokenization_ms: u64) -> Self {
        Self {
            provider_prompt_render_ms,
            exact_tokenization_ms,
        }
    }

    /// Time spent asking the provider to render its exact native prompt.
    #[must_use]
    pub const fn provider_prompt_render_ms(self) -> u64 {
        self.provider_prompt_render_ms
    }

    /// Time spent counting the rendered prompt with the selected model tokenizer.
    #[must_use]
    pub const fn exact_tokenization_ms(self) -> u64 {
        self.exact_tokenization_ms
    }
}

/// Exact content-free admission facts paired with one prepared chat prompt.
pub struct PreparedChatGeneration {
    /// Request-local provider continuation, committed only after a successful round.
    pub prompt_continuation: Option<crate::ChatPromptContinuation>,
    /// Adapter-private prompt supplied only to the inference boundary.
    pub prompt: PreparedChatPrompt,
    /// Opaque managed image identifiers in prompt order.
    pub asset_ids: Vec<AssetId>,
    /// Immutable selected model and provider binding.
    pub binding: GenerationBinding,
    /// Configured context window for this round.
    pub context_tokens: u32,
    /// Configured output ceiling for this round.
    pub output_tokens: u32,
    /// Exact tokenizer-derived admitted prompt tokens, excluding image tokens.
    pub admitted_prompt_tokens: u64,
    /// Content-free diagnostic timings for exact prompt admission.
    pub prompt_preparation_timing: PromptPreparationTiming,
    /// Exact provider-rendered prompt retained transiently for prefix eligibility.
    ///
    /// This host-only value must never be persisted or included in diagnostics.
    pub rendered_provider_prompt: String,
    /// Versioned host-selected model-neutral execution profile.
    pub execution_profile: ExecutionProfileDecision,
    /// Adapter-neutral reasoning directive translated privately by the adapter.
    pub reasoning_strength: ReasoningStrength,
    /// Profile-default output cap before a final-answer preservation override.
    pub profile_output_tokens: u32,
    /// Context capacity reserved for private reasoning in this round.
    pub reasoning_reserve_tokens: u64,
    /// Whether the output cap was raised to preserve the final-answer reserve.
    pub final_answer_output_override: bool,
    /// Whether exact untrusted extracted evidence replaced rich-input bytes.
    pub rich_evidence_checkpoint_reused: bool,
    /// Number of selected history messages, independent of tool schema bytes.
    pub selected_history_messages: usize,
    /// Complete historical prefix covered by the narrative checkpoint.
    pub compacted_history_messages: usize,
    /// Whether this exact admission committed a forward dialogue boundary.
    pub dialogue_checkpoint_advanced: bool,
    /// Content-free narrative generation/reuse/fallback disposition.
    pub narrative_checkpoint: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ExecutionRoundBudget {
    pub(super) output_tokens: u32,
    pub(super) profile_output_tokens: u32,
    pub(super) reasoning_reserve_tokens: u64,
    pub(super) final_answer_output_override: bool,
}

/// Host-owned operation counters projected into one model generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ToolOperationBudget {
    used_batches: usize,
    remaining_batches: usize,
    used_calls: usize,
    remaining_calls: usize,
    consecutive_discovery_calls: usize,
    suggested_execution_reserve: usize,
}

impl ToolOperationBudget {
    /// Creates trusted bounded counters for one model generation.
    #[must_use]
    pub const fn new(
        used_batches: usize,
        remaining_batches: usize,
        used_calls: usize,
        remaining_calls: usize,
        consecutive_discovery_calls: usize,
        suggested_execution_reserve: usize,
    ) -> Self {
        Self {
            used_batches,
            remaining_batches,
            used_calls,
            remaining_calls,
            consecutive_discovery_calls,
            suggested_execution_reserve,
        }
    }
}

/// Host-derived planning state projected without suppressing workspace tools.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolOrchestrationPhase {
    /// Answer from bounded public-web evidence without semantic-stage ceremony.
    WebAnswer,
    /// No provisional plan has been accepted.
    EvidenceCollection,
    /// Legacy state retained for persisted compatibility.
    EvidenceRefinement,
    /// A provisional plan has been accepted and may be revised.
    Execution,
}

/// Closed host-owned state for one semantic execution round.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolExecutionState {
    /// Decide and admit the semantic execution contract.
    Planning,
    /// Produce and validate the advisory execution forecast before task operations.
    ExecutionForecast,
    /// Advance one dependency-ready semantic objective.
    CurrentStepExecution,
    /// Correct a typed defect without changing the accepted objective.
    SameStepRepair,
    /// Resolve a bounded evidence gap and return to its suspended parent step.
    BoundedRefinement,
    /// Replace unfinished semantic work through a versioned revision.
    PlanRevision,
    /// Stop effects until the user supplies a required choice or authority.
    AwaitingUser,
    /// Compare trusted receipts and postconditions with the original outcome.
    Verification,
    /// Report a verified or otherwise established result without another tool.
    Completion,
}

impl ToolExecutionState {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Planning => "planning",
            Self::ExecutionForecast => "execution_forecast",
            Self::CurrentStepExecution => "current_step_execution",
            Self::SameStepRepair => "same_step_repair",
            Self::BoundedRefinement => "bounded_refinement",
            Self::PlanRevision => "plan_revision",
            Self::AwaitingUser => "awaiting_user",
            Self::Verification => "verification",
            Self::Completion => "completion",
        }
    }
}

/// Host-observed progress of one model-authored forecast step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolPlanStepStatus {
    /// No forecasted operation in the step has completed.
    NotStarted,
    /// At least one forecasted operation completed and work remains.
    InProgress,
    /// Every forecasted operation completed; semantic sufficiency is not implied.
    ForecastComplete,
    /// The next matching forecasted operation failed and may need repair or revision.
    Blocked,
}

/// Bounded host projection of one forecast step.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolPlanStepProgress {
    ordinal: usize,
    status: ToolPlanStepStatus,
    remaining_tools: Vec<String>,
    id: Option<String>,
    depends_on: Vec<String>,
    purpose: Option<String>,
    required_evidence: Vec<String>,
    completion_evidence: Option<String>,
    completed_call_ids: Vec<String>,
    forecast_category: Option<String>,
}

impl ToolPlanStepProgress {
    /// Creates one step projection from registry-validated tool names.
    #[must_use]
    pub const fn new(
        ordinal: usize,
        status: ToolPlanStepStatus,
        remaining_tools: Vec<String>,
    ) -> Self {
        Self {
            ordinal,
            status,
            remaining_tools,
            id: None,
            depends_on: Vec::new(),
            purpose: None,
            required_evidence: Vec::new(),
            completion_evidence: None,
            completed_call_ids: Vec::new(),
            forecast_category: None,
        }
    }

    /// Adds the validated, bounded contract retained from the accepted plan.
    #[must_use]
    pub fn with_contract(
        mut self,
        id: String,
        purpose: String,
        required_evidence: Vec<String>,
        completion_evidence: String,
        completed_call_ids: Vec<String>,
    ) -> Self {
        self.id = Some(id);
        self.purpose = Some(purpose);
        self.required_evidence = required_evidence;
        self.completion_evidence = Some(completion_evidence);
        self.completed_call_ids = completed_call_ids;
        self
    }

    /// Adds the exact validated semantic dependencies for focused receipt
    /// selection. Legacy sequential plans may supply their synthesized parent.
    #[must_use]
    pub fn with_dependencies(mut self, depends_on: Vec<String>) -> Self {
        self.depends_on = depends_on;
        self
    }

    /// Adds the advisory category retained from the accepted execution forecast.
    #[must_use]
    pub fn with_forecast_category(mut self, category: Option<String>) -> Self {
        self.forecast_category = category;
        self
    }
}

/// Typed outcome declared by an accepted provisional plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolPlanOutcome {
    /// Produce a text answer after any forecasted evidence work.
    Answer,
    /// Create and verify a workspace file.
    CreateFile,
    /// Modify and verify a workspace file.
    ModifyFile,
    /// Perform another task whose semantics are not host-verifiable.
    Other,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum ToolPlanDeviationState {
    #[default]
    None,
    RecoveryRequired,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum ToolPlanRecoveryState {
    #[default]
    None,
    Required,
    LengthExhausted,
}

/// Host-owned forecast cursor injected into the next generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolPlanProgress {
    revision: usize,
    completed_before_revision: usize,
    steps: Vec<ToolPlanStepProgress>,
    suggested_step: Option<usize>,
    outcome: ToolPlanOutcome,
    outcome_satisfied: bool,
    recovery_state: ToolPlanRecoveryState,
    adaptive_calls_remaining: usize,
    outcome_reserve_available: bool,
    unforecasted_operations: usize,
    deviation_state: ToolPlanDeviationState,
    retained_call_ids: BTreeSet<String>,
    accepted_target: Option<String>,
    accepted_constraints: Vec<String>,
    accepted_success_criteria: Vec<String>,
}

impl ToolPlanProgress {
    /// Creates a bounded progress projection from validated plan and ledger state.
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "the constructor mirrors every field in the closed plan-progress projection"
    )]
    pub const fn new(
        revision: usize,
        completed_before_revision: usize,
        steps: Vec<ToolPlanStepProgress>,
        suggested_step: Option<usize>,
        outcome: ToolPlanOutcome,
        outcome_satisfied: bool,
        recovery_required: bool,
        adaptive_calls_remaining: usize,
        outcome_reserve_available: bool,
        unforecasted_operations: usize,
    ) -> Self {
        Self {
            revision,
            completed_before_revision,
            steps,
            suggested_step,
            outcome,
            outcome_satisfied,
            recovery_state: if recovery_required {
                ToolPlanRecoveryState::Required
            } else {
                ToolPlanRecoveryState::None
            },
            adaptive_calls_remaining,
            outcome_reserve_available,
            unforecasted_operations,
            deviation_state: ToolPlanDeviationState::None,
            retained_call_ids: BTreeSet::new(),
            accepted_target: None,
            accepted_constraints: Vec::new(),
            accepted_success_criteria: Vec::new(),
        }
    }

    /// Adds the bounded outcome contract retained from semantic plan admission.
    #[must_use]
    pub fn with_outcome_contract(
        mut self,
        target: String,
        constraints: Vec<String>,
        success_criteria: Vec<String>,
    ) -> Self {
        self.accepted_target = Some(target);
        self.accepted_constraints = constraints;
        self.accepted_success_criteria = success_criteria;
        self
    }

    /// Retains the smallest host-selected set of evidence calls needed to
    /// refine unfinished work, including useful results from an earlier plan
    /// revision.
    #[must_use]
    pub fn with_retained_call_ids(mut self, call_ids: impl IntoIterator<Item = String>) -> Self {
        self.retained_call_ids.extend(call_ids);
        self
    }

    /// Marks that the next decision must return to the forecast or revise it.
    #[must_use]
    pub const fn with_deviation_recovery(mut self, required: bool) -> Self {
        self.deviation_state = if required {
            ToolPlanDeviationState::RecoveryRequired
        } else {
            ToolPlanDeviationState::None
        };
        self
    }

    /// Marks that outcome recovery follows a no-action output-limit completion.
    #[must_use]
    pub const fn with_length_exhausted_recovery(mut self, required: bool) -> Self {
        self.recovery_state = if required {
            ToolPlanRecoveryState::LengthExhausted
        } else {
            ToolPlanRecoveryState::None
        };
        self
    }

    #[cfg(test)]
    fn suggested_step_is_execution_ready(&self) -> bool {
        self.suggested_step
            .and_then(|ordinal| self.steps.get(ordinal.saturating_sub(1)))
            .is_some_and(|step| {
                step.status != ToolPlanStepStatus::Blocked
                    && !step.remaining_tools.is_empty()
                    && step.remaining_tools.iter().all(|tool| {
                        matches!(
                            tool.as_str(),
                            "filesystem.write_text"
                                | "filesystem.replace_text"
                                | "filesystem.write_delimited_table"
                                | "orchestration.transform_artifact"
                        )
                    })
            })
    }

    #[cfg(test)]
    fn final_response_ready(&self) -> bool {
        !self.outcome_satisfied
            && self.suggested_step.is_none()
            && matches!(
                self.outcome,
                ToolPlanOutcome::Answer | ToolPlanOutcome::Other
            )
            && self
                .steps
                .iter()
                .all(|step| step.status == ToolPlanStepStatus::ForecastComplete)
    }
}

/// Host-owned operation state projected into one model generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolOrchestrationContext {
    budget: ToolOperationBudget,
    workspace_mutation_tools_available: bool,
    phase: ToolOrchestrationPhase,
    execution_state: ToolExecutionState,
    plan_progress: Option<ToolPlanProgress>,
    phase_directive: Option<ToolPhaseDirective>,
    recovery_diagnostic: Option<ToolRecoveryDiagnostic>,
    active_task_references: Vec<ToolTaskReference>,
}

/// Bounded non-authoritative task state retained by the host across focused rounds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolTaskReference {
    kind: &'static str,
    value: String,
    source: &'static str,
    source_call_id: Option<String>,
    tool: Option<&'static str>,
    argument: Option<&'static str>,
    next_operation: Option<&'static str>,
    required_alongside: Vec<&'static str>,
}

impl ToolTaskReference {
    /// Retains an exact URL supplied by the operator without treating it as authority.
    #[must_use]
    pub fn user_supplied_url(value: String) -> Option<Self> {
        valid_task_reference(&value).then_some(Self {
            kind: "user_supplied_url",
            value,
            source: "operator_message",
            source_call_id: None,
            tool: None,
            argument: None,
            next_operation: None,
            required_alongside: Vec::new(),
        })
    }

    /// Retains an opaque continuation produced and verified by a host tool.
    #[must_use]
    pub fn tool_continuation(value: String) -> Option<Self> {
        valid_task_reference(&value).then_some(Self {
            kind: "tool_continuation",
            value,
            source: "trusted_tool_result",
            source_call_id: None,
            tool: None,
            argument: None,
            next_operation: None,
            required_alongside: Vec::new(),
        })
    }

    /// Retains one Bazaar-discovered resource and its exact model-facing use.
    #[must_use]
    pub fn x402_resource(value: String, source_call_id: String) -> Option<Self> {
        (valid_task_reference(&value) && valid_task_reference(&source_call_id)).then_some(Self {
            kind: "x402_resource",
            value,
            source: "trusted_tool_result",
            source_call_id: Some(source_call_id),
            tool: Some("x402.request"),
            argument: Some("resourceRef"),
            next_operation: Some("purchase"),
            required_alongside: vec!["input", "reason"],
        })
    }
}

fn valid_task_reference(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 2_048
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b':' | b'/' | b'?' | b'.' | b'_' | b'-' | b'=' | b'&' | b'%' | b'#' | b'+'
                )
        })
}

/// Concise host-owned repair state projected into exactly the next semantic step.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolRecoveryDiagnostic {
    kind: String,
    message: String,
    attempt: usize,
    max_attempts: usize,
}

impl ToolRecoveryDiagnostic {
    /// Creates a bounded model-visible diagnostic without granting authority.
    #[must_use]
    pub const fn new(kind: String, message: String, attempt: usize, max_attempts: usize) -> Self {
        Self {
            kind,
            message,
            attempt,
            max_attempts,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ToolPhaseContract {
    name: String,
    allowed_top_level_arguments: Vec<String>,
    required_top_level_arguments: Vec<String>,
}

/// Compact provider-neutral index of one stable tool catalogue category.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolCategoryIndexEntry {
    name: String,
    responsibility: String,
    operations: Vec<String>,
}

impl ToolCategoryIndexEntry {
    /// Creates a bounded descriptive category entry. Category metadata is
    /// documentation only and grants no operation authority.
    #[must_use]
    pub const fn new(name: String, responsibility: String, operations: Vec<String>) -> Self {
        Self {
            name,
            responsibility,
            operations,
        }
    }

    /// Stable category identifier.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Plain-language category responsibility.
    #[must_use]
    pub fn responsibility(&self) -> &str {
        &self.responsibility
    }

    /// Available operation names in this category.
    #[must_use]
    pub fn operations(&self) -> &[String] {
        &self.operations
    }
}

/// Host-classified scope for one generation round.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolPhaseScope {
    /// Evidence or repair may still reveal a concrete dependency.
    Adaptive {
        /// Whether a concrete blocker may be expressed as a plan revision.
        revision_available: bool,
    },
    /// Only the accepted outcome-bearing operation may execute.
    ExecutionReady,
    /// The accepted forecast is complete and the assistant must answer normally.
    FinalResponseReady,
}

/// Closed host-verifiable result required before a bounded stage receives
/// progress credit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolStagePostcondition {
    /// A bounded UTF-8 read returned structured content.
    TextLoaded,
    /// A validated image was imported as a managed conversation asset.
    ImageLoaded,
    /// A typed computation completed and produced a reusable result receipt.
    ComputationCompleted,
    /// Every applied file change passed the normal broker's write verification.
    FileWriteVerified,
    /// Directory creation passed the normal broker's post-create verification.
    DirectoryCreateVerified,
}

impl ToolStagePostcondition {
    const fn as_str(self) -> &'static str {
        match self {
            Self::TextLoaded => "structured_text_loaded",
            Self::ImageLoaded => "managed_image_loaded",
            Self::ComputationCompleted => "typed_computation_completed",
            Self::FileWriteVerified => "broker_file_write_verified",
            Self::DirectoryCreateVerified => "broker_directory_create_verified",
        }
    }
}

/// Trusted projection of one predictable, dependency-ready plan stage.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolBoundedStageContract {
    operation: String,
    postcondition: ToolStagePostcondition,
    retry_limit: usize,
    evidence_call_ids: Vec<String>,
}

impl ToolBoundedStageContract {
    /// Creates a provider-neutral contract from host-proven readiness facts.
    #[must_use]
    pub fn new(
        operation: String,
        postcondition: ToolStagePostcondition,
        retry_limit: usize,
        evidence_call_ids: Vec<String>,
    ) -> Self {
        Self {
            operation,
            postcondition,
            retry_limit,
            evidence_call_ids,
        }
    }
}

impl ToolPhaseScope {
    const fn revision_available(self) -> bool {
        matches!(
            self,
            Self::Adaptive {
                revision_available: true
            } | Self::ExecutionReady
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ToolPhaseDirective {
    decision: ExecutionProfileDecision,
    tools: Vec<ToolPhaseContract>,
    category_index: Vec<ToolCategoryIndexEntry>,
    forced_replanning_allowed: bool,
    scope: ToolPhaseScope,
    bounded_stage: Option<ToolBoundedStageContract>,
}

impl ToolOrchestrationContext {
    /// Creates a trusted bounded operation-budget projection.
    #[must_use]
    pub const fn new(
        budget: ToolOperationBudget,
        workspace_mutation_tools_available: bool,
        phase: ToolOrchestrationPhase,
    ) -> Self {
        Self {
            budget,
            workspace_mutation_tools_available,
            phase,
            execution_state: if matches!(phase, ToolOrchestrationPhase::EvidenceCollection) {
                ToolExecutionState::Planning
            } else {
                ToolExecutionState::CurrentStepExecution
            },
            plan_progress: None,
            phase_directive: None,
            recovery_diagnostic: None,
            active_task_references: Vec::new(),
        }
    }

    /// Sets the exact host-derived execution state for this round.
    #[must_use]
    pub const fn with_execution_state(mut self, state: ToolExecutionState) -> Self {
        self.execution_state = state;
        self
    }

    /// Adds the current host-observed plan cursor without changing tool authority.
    #[must_use]
    pub fn with_plan_progress(mut self, progress: Option<ToolPlanProgress>) -> Self {
        self.plan_progress = progress;
        self
    }

    /// Adds the exact typed defect that the current semantic step must repair.
    #[must_use]
    pub fn with_recovery_diagnostic(mut self, diagnostic: Option<ToolRecoveryDiagnostic>) -> Self {
        self.recovery_diagnostic = diagnostic;
        self
    }

    /// Adds bounded location and continuation facts that remain relevant after
    /// focused history removes older conversation turns. These values grant no
    /// capability, permission or approval.
    #[must_use]
    pub fn with_active_task_references(mut self, references: Vec<ToolTaskReference>) -> Self {
        self.active_task_references = references.into_iter().take(8).collect();
        self
    }

    /// Adds a concise, catalogue-derived contract for the operation expected in
    /// this round. This is a reminder only; the advertised JSON Schemas remain
    /// the source of truth and tool authority is unchanged.
    #[must_use]
    pub fn with_phase_directive(
        mut self,
        decision: ExecutionProfileDecision,
        catalogue: &[PromptToolDefinition],
        category_index: &[ToolCategoryIndexEntry],
        relevant_tool_names: &[&str],
        forced_replanning_allowed: bool,
        scope: ToolPhaseScope,
    ) -> Self {
        let mut tools = catalogue
            .iter()
            .filter(|tool| relevant_tool_names.contains(&tool.name.as_str()))
            .map(|tool| {
                let mut allowed_top_level_arguments = tool.parameters["properties"]
                    .as_object()
                    .map(|properties| properties.keys().cloned().collect::<Vec<_>>())
                    .unwrap_or_default();
                allowed_top_level_arguments.sort();
                let mut required_top_level_arguments = tool.parameters["required"]
                    .as_array()
                    .map(|required| {
                        required
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .map(str::to_owned)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                required_top_level_arguments.sort();
                ToolPhaseContract {
                    name: tool.name.clone(),
                    allowed_top_level_arguments,
                    required_top_level_arguments,
                }
            })
            .collect::<Vec<_>>();
        tools.sort_by(|left, right| left.name.cmp(&right.name));
        self.phase_directive = Some(ToolPhaseDirective {
            decision,
            tools,
            category_index: category_index.to_vec(),
            forced_replanning_allowed: forced_replanning_allowed && scope.revision_available(),
            scope,
            bounded_stage: None,
        });
        self
    }

    /// Adds a bounded stage contract after readiness was proven from trusted
    /// plan state. This narrows routing only; normal permission mediation and
    /// broker postcondition verification remain authoritative.
    #[must_use]
    pub fn with_bounded_stage_contract(
        mut self,
        contract: Option<ToolBoundedStageContract>,
    ) -> Self {
        if let Some(directive) = &mut self.phase_directive {
            directive.bounded_stage = contract;
        }
        self
    }

    pub(super) fn focused_history_call_ids(&self) -> Option<BTreeSet<String>> {
        let mut call_ids = self
            .plan_progress
            .as_ref()
            .map_or_else(BTreeSet::new, ToolPlanProgress::dependency_call_ids);
        call_ids.extend(
            self.active_task_references
                .iter()
                .filter_map(|reference| reference.source_call_id.clone()),
        );
        (!call_ids.is_empty()).then_some(call_ids)
    }
}

mod projection;
