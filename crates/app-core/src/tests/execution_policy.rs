use super::*;

#[test]
fn computation_adapter_diagnostics_use_only_closed_stage_coordinates() {
    let cases = [
        (
            ComputationDiagnosticStage::PrepareResult,
            (Component::Computation, Operation::Validate),
            "prepare_result",
        ),
        (
            ComputationDiagnosticStage::PersistResult,
            (Component::Storage, Operation::Persist),
            "persist_result",
        ),
        (
            ComputationDiagnosticStage::ReturnAgentResult,
            (Component::Agent, Operation::ExecuteTool),
            "return_agent_result",
        ),
        (
            ComputationDiagnosticStage::ProjectIpcResult,
            (Component::Application, Operation::Validate),
            "project_ipc_result",
        ),
    ];
    for (stage, coordinates, name) in cases {
        assert_eq!(stage.diagnostic_coordinates(), coordinates);
        assert_eq!(stage.name(), name);
    }
}

#[test]
fn operation_budget_projection_warns_after_repeated_discovery() {
    let early = ToolOrchestrationContext::new(
        ToolOperationBudget::new(2, 10, 3, 15, 3, 2),
        true,
        ToolOrchestrationPhase::EvidenceCollection,
    )
    .render();
    assert!(early.contains("Calls used: 3; calls remaining: 15"));
    assert!(!early.contains("Warning:"));
    assert!(early.contains("Workspace mutation tools available: yes"));
    assert!(early.contains("Orchestration phase: semantic execution plan required"));

    let warning = ToolOrchestrationContext::new(
        ToolOperationBudget::new(4, 8, 4, 14, 4, 2),
        true,
        ToolOrchestrationPhase::EvidenceRefinement,
    )
    .render();
    assert!(warning.contains("4 consecutive read-only discovery calls"));
    assert!(warning.contains("Suggested reserve for execution and validation: 2"));
    assert!(warning.contains("filesystem.collect_evidence"));
    assert!(warning.contains("Orchestration phase: evidence refinement"));

    let read_only = ToolOrchestrationContext::new(
        ToolOperationBudget::new(0, 12, 0, 18, 0, 1),
        false,
        ToolOrchestrationPhase::EvidenceCollection,
    )
    .render();
    assert!(read_only.contains("Workspace mutation tools available: no"));
    assert!(read_only.contains("Project settings > Folder access > Read and write"));

    let execution = ToolOrchestrationContext::new(
        ToolOperationBudget::new(1, 11, 1, 17, 1, 2),
        true,
        ToolOrchestrationPhase::Execution,
    )
    .render();
    assert!(execution.contains("Orchestration phase: semantic execution plan active"));
    assert!(execution.contains("does not grant permissions"));
    let execution_tail = ToolOrchestrationContext::new(
        ToolOperationBudget::new(1, 11, 1, 17, 1, 2),
        false,
        ToolOrchestrationPhase::Execution,
    )
    .render_tail();
    assert!(execution_tail.contains("\"workspaceMutationToolsAvailable\":false"));
    assert!(!execution_tail.contains("\"mutationToolsAvailable\""));

    let progress = ToolPlanProgress::new(
        2,
        1,
        vec![
            ToolPlanStepProgress::new(1, ToolPlanStepStatus::ForecastComplete, Vec::new()),
            ToolPlanStepProgress::new(
                2,
                ToolPlanStepStatus::NotStarted,
                vec!["filesystem.write_text".to_owned()],
            ),
        ],
        Some(2),
        ToolPlanOutcome::CreateFile,
        false,
        true,
        2,
        true,
        1,
    );
    let recovery = ToolOrchestrationContext::new(
        ToolOperationBudget::new(4, 8, 6, 12, 0, 2),
        true,
        ToolOrchestrationPhase::Execution,
    )
    .with_plan_progress(Some(progress.clone().with_deviation_recovery(true)))
    .render();
    assert!(recovery.contains("Stage 1: status=forecast_complete"));
    assert!(recovery.contains("Current plan stage: 2"));
    assert!(recovery.contains("filesystem.write_text"));
    assert!(recovery.contains("host verification: unsatisfied"));
    assert!(
        recovery
            .contains("Remaining global execution allowance represented by this plan: 2 call(s)")
    );
    assert!(recovery.contains("non-suggested operations observed: 1"));
    assert!(recovery.contains("without treating its suggestions as an allowlist"));
    assert!(recovery.contains("Outcome recovery is required"));
    assert!(recovery.contains("Scope deviation recovery is required now"));
    assert!(recovery.contains("smallest useful operation"));
    assert!(recovery.contains("does not authorize revision or additional evidence"));
    assert!(recovery.contains("Direct execution required"));
    assert!(recovery.contains("emit the advertised operation in this response"));

    let constrained = ToolOrchestrationContext::new(
        ToolOperationBudget::new(5, 7, 6, 12, 0, 2),
        true,
        ToolOrchestrationPhase::Execution,
    )
    .with_plan_progress(Some(progress.with_length_exhausted_recovery(true)))
    .render();
    assert!(constrained.contains("Constrained action recovery is required"));
    assert!(constrained.contains("Do not repeat its analysis"));
    assert!(constrained.contains("next advertised stage control or task operation"));
    assert!(constrained.contains("structured output in tool arguments"));
    assert!(!constrained.contains("outstanding outcome-bearing operation"));
    assert!(constrained.contains("Recovery is bounded"));
}

#[test]
fn execution_policy_is_stable_while_round_state_moves_to_the_tail() {
    let first = ToolOrchestrationContext::new(
        ToolOperationBudget::new(1, 11, 2, 16, 0, 2),
        true,
        ToolOrchestrationPhase::Execution,
    )
    .with_execution_state(ToolExecutionState::CurrentStepExecution);
    let later = ToolOrchestrationContext::new(
        ToolOperationBudget::new(7, 5, 10, 8, 3, 1),
        true,
        ToolOrchestrationPhase::Execution,
    )
    .with_execution_state(ToolExecutionState::SameStepRepair)
    .with_recovery_diagnostic(Some(ToolRecoveryDiagnostic::new(
        "invalid_arguments".to_owned(),
        "Remove one unsupported field.".to_owned(),
        1,
        3,
    )));

    assert_eq!(first.stable_policy(), later.stable_policy());
    assert_ne!(first.render_tail(), later.render_tail());
    assert!(first.render_tail().contains("execution_tail_capsule_v1"));
    assert!(first.render_tail().contains("\"roundsRemaining\":11"));
    assert!(
        later
            .render_tail()
            .contains("Remove one unsupported field.")
    );
    assert!(!first.stable_policy().contains("roundsRemaining"));
    assert!(!first.stable_policy().contains("invalid_arguments"));
}

#[test]
fn completed_answer_forecast_requires_a_tool_free_final_response() {
    let final_response = ToolPlanProgress::new(
        1,
        0,
        vec![ToolPlanStepProgress::new(
            1,
            ToolPlanStepStatus::ForecastComplete,
            Vec::new(),
        )],
        None,
        ToolPlanOutcome::Answer,
        false,
        false,
        1,
        false,
        0,
    );
    let final_context = ToolOrchestrationContext::new(
        ToolOperationBudget::new(1, 11, 1, 17, 0, 2),
        true,
        ToolOrchestrationPhase::Execution,
    )
    .with_execution_state(ToolExecutionState::Completion)
    .with_phase_directive(
        ExecutionProfileDecision {
            profile: ExecutionProfile::Latency,
            work: ExecutionWorkKind::FinalResponse,
            policy_version: agent_core::EXECUTION_PROFILE_POLICY_VERSION,
            bounded_action: false,
            bulk_arguments: false,
        },
        &[],
        &[],
        &[],
        false,
        ToolPhaseScope::FinalResponseReady,
    )
    .with_plan_progress(Some(final_response))
    .render();
    assert!(final_context.contains("accepted tool forecast is complete"));
    assert!(final_context.contains("without further tool calls"));
    assert!(final_context.contains("tool surface is intentionally closed"));
    assert!(final_context.contains("respond to the user now"));
    assert!(final_context.contains("Do not call any task or orchestration tool"));
}

#[test]
fn phase_directive_is_catalogue_derived_and_reports_execution_scope() {
    let tool = PromptToolDefinition {
        name: "orchestration.submit_plan".to_owned(),
        description: "Revise plan".to_owned(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {"mode": {"type": "string"}, "steps": {"type": "array"}},
            "required": ["mode", "steps"],
            "additionalProperties": false
        }),
    };
    let context = ToolOrchestrationContext::new(
        ToolOperationBudget::new(1, 11, 1, 17, 0, 2),
        true,
        ToolOrchestrationPhase::Execution,
    )
    .with_phase_directive(
        ExecutionProfileDecision {
            profile: ExecutionProfile::Balanced,
            work: ExecutionWorkKind::Recovery,
            policy_version: agent_core::EXECUTION_PROFILE_POLICY_VERSION,
            bounded_action: false,
            bulk_arguments: false,
        },
        &[tool],
        &[ToolCategoryIndexEntry::new(
            "orchestration".to_owned(),
            "Declare or revise orchestration contracts.".to_owned(),
            vec!["orchestration.submit_plan".to_owned()],
        )],
        &["orchestration.submit_plan"],
        false,
        ToolPhaseScope::ExecutionReady,
    )
    .with_bounded_stage_contract(Some(ToolBoundedStageContract::new(
        "orchestration.submit_plan".to_owned(),
        ToolStagePostcondition::ComputationCompleted,
        2,
        vec!["receipt-a".to_owned()],
    )));
    let stable_catalogue = context.stable_catalogue_index().unwrap_or_default();
    assert!(stable_catalogue.contains("tool_category_index_v1"));
    assert!(stable_catalogue.contains("orchestration.submit_plan"));
    assert!(!stable_catalogue.contains("invalid_arguments"));
    let rendered = context.render();

    assert!(rendered.contains("phase_directive"));
    assert!(rendered.contains("toolCategoryIndex"));
    assert!(rendered.contains("Declare or revise orchestration contracts."));
    assert!(rendered.contains("allowedTopLevelArguments"));
    assert!(rendered.contains("orchestration.submit_plan"));
    assert!(rendered.contains("No execution forecast"));
    assert!(rendered.contains("obligation receipts are required"));
    assert!(rendered.contains("No orchestration control grants permission"));
    assert!(rendered.contains("not_needed"));
    assert!(rendered.contains("execution_ready"));
    assert!(rendered.contains("revise_only_for_a_concrete_unresolved_dependency"));
    assert!(rendered.contains("typed_computation_completed"));
    assert!(rendered.contains("receipt-a"));
    assert!(rendered.contains("revise_plan"));
    assert!(rendered.contains("ask_user"));
    assert!(rendered.contains("did not certify the model's interpretation"));
}

#[test]
fn recovery_checkpoint_projects_one_typed_same_step_diagnostic() {
    let rendered = ToolOrchestrationContext::new(
        ToolOperationBudget::new(2, 10, 3, 15, 0, 2),
        true,
        ToolOrchestrationPhase::Execution,
    )
    .with_execution_state(ToolExecutionState::SameStepRepair)
    .with_recovery_diagnostic(Some(ToolRecoveryDiagnostic::new(
        "invalid_arguments".to_owned(),
        "The write call used one unsupported field.".to_owned(),
        1,
        3,
    )))
    .render();

    assert!(rendered.contains("recovery_checkpoint"));
    assert!(rendered.contains("invalid_arguments"));
    assert!(rendered.contains("same semantic step"));
    assert!(rendered.contains("Do not restart discovery"));
    assert!(rendered.contains("\"attempt\":1"));
    assert!(rendered.contains("\"maxAttempts\":3"));
}

#[test]
fn focused_capsule_projects_closed_state_plan_dependencies_and_final_review_contract() {
    let states = [
        ToolExecutionState::Planning,
        ToolExecutionState::ExecutionForecast,
        ToolExecutionState::CurrentStepExecution,
        ToolExecutionState::SameStepRepair,
        ToolExecutionState::BoundedRefinement,
        ToolExecutionState::PlanRevision,
        ToolExecutionState::AwaitingUser,
        ToolExecutionState::Verification,
        ToolExecutionState::Completion,
    ];
    for state in states {
        let rendered = ToolOrchestrationContext::new(
            ToolOperationBudget::new(1, 11, 1, 17, 0, 2),
            true,
            ToolOrchestrationPhase::Execution,
        )
        .with_execution_state(state)
        .render();
        assert!(rendered.contains(state.as_str()));
    }

    let source = ToolPlanStepProgress::new(1, ToolPlanStepStatus::ForecastComplete, Vec::new())
        .with_contract(
            "source".to_owned(),
            "Load source".to_owned(),
            vec!["semantic_content".to_owned()],
            "Source loaded".to_owned(),
            vec!["source-call".to_owned()],
        );
    let write = ToolPlanStepProgress::new(
        2,
        ToolPlanStepStatus::NotStarted,
        vec!["filesystem.write_text".to_owned()],
    )
    .with_contract(
        "write".to_owned(),
        "Write the requested table".to_owned(),
        Vec::new(),
        "CSV exists and the write postcondition is verified".to_owned(),
        Vec::new(),
    )
    .with_dependencies(vec!["source".to_owned()]);
    let progress = ToolPlanProgress::new(
        1,
        0,
        vec![source, write],
        Some(2),
        ToolPlanOutcome::CreateFile,
        false,
        false,
        2,
        true,
        0,
    )
    .with_outcome_contract(
        "Create the requested CSV".to_owned(),
        vec!["Preserve source values".to_owned()],
        vec!["Verified CSV exists".to_owned()],
    );
    let context = ToolOrchestrationContext::new(
        ToolOperationBudget::new(1, 11, 1, 17, 0, 2),
        true,
        ToolOrchestrationPhase::Execution,
    )
    .with_execution_state(ToolExecutionState::CurrentStepExecution)
    .with_plan_progress(Some(progress));
    let rendered = context.clone().render();
    assert_eq!(rendered, context.render());
    assert!(rendered.contains("current_step_capsule"));
    assert!(rendered.contains("focused_execution_capsule_v5"));
    assert!(rendered.contains("capsuleSha256"));
    assert!(rendered.contains("stageProgress"));
    assert!(rendered.contains("tokenBudgetRule"));
    assert!(rendered.contains("Create the requested CSV"));
    assert!(rendered.contains("Verified CSV exists"));
    assert!(rendered.contains("source-call"));
    assert!(rendered.contains("original user instruction"));
    assert!(rendered.contains("successful tool transport alone is not task completion"));
    assert!(rendered.contains("Suggested tools are promoted starting points"));
    assert!(rendered.contains("a specific missing fact"));
    assert!(rendered.contains("does not assert that any fact is missing"));
    assert!(rendered.contains("Use arithmetic only to detect a possible misread"));
    assert!(rendered.contains("position as the stage boundary"));
    assert!(rendered.contains("newly ready stages"));
    assert!(rendered.contains("Learned facts and changed tool preferences"));
}

#[test]
fn execution_budget_preserves_final_answer_without_raising_intermediate_profiles() {
    let latency_mutation = ExecutionProfileDecision {
        profile: ExecutionProfile::Latency,
        work: agent_core::ExecutionWorkKind::Mutation,
        policy_version: agent_core::EXECUTION_PROFILE_POLICY_VERSION,
        bounded_action: false,
        bulk_arguments: false,
    };
    assert_eq!(
        execution_round_budget(latency_mutation, 8_192, 6_144),
        ExecutionRoundBudget {
            output_tokens: 2_048,
            profile_output_tokens: 2_048,
            reasoning_reserve_tokens: 512,
            final_answer_output_override: false,
        }
    );

    let latency_final = ExecutionProfileDecision {
        work: agent_core::ExecutionWorkKind::FinalResponse,
        ..latency_mutation
    };
    assert_eq!(
        execution_round_budget(latency_final, 8_192, 6_144),
        ExecutionRoundBudget {
            output_tokens: 6_144,
            profile_output_tokens: 2_048,
            reasoning_reserve_tokens: 512,
            final_answer_output_override: true,
        }
    );

    let bounded_action = latency_mutation.for_bounded_action();
    assert_eq!(
        execution_round_budget(bounded_action, 8_192, 6_144),
        ExecutionRoundBudget {
            output_tokens: 4_096,
            profile_output_tokens: 4_096,
            reasoning_reserve_tokens: 512,
            final_answer_output_override: false,
        }
    );
}

#[test]
fn execution_profiles_translate_to_adapter_neutral_reasoning_strength() {
    assert_eq!(
        reasoning_strength(ExecutionProfile::Latency),
        ReasoningStrength::Disabled
    );
    assert_eq!(
        reasoning_strength(ExecutionProfile::Balanced),
        ReasoningStrength::Medium
    );
    assert_eq!(
        reasoning_strength(ExecutionProfile::Deep),
        ReasoningStrength::High
    );
}
