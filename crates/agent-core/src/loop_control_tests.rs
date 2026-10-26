use std::time::Duration;

use super::*;
use crate::{
    ApprovalRequirement, ExecutionBudgets, ExecutionLedger, LedgerCapability, LedgerNextAction,
    LedgerRequest, LedgerTerminalState, NewMutationChange, NewObservation, ObservedValues,
    PermissionBinding, PermissionResourceKind, PermissionResourceScope, ProjectCapability,
    ResultContent, RiskLevel, SupportedResultType, ToolClassification, ToolDefinition, ToolFailure,
    ToolLimits, ToolRegistry, ToolRegistryError, ToolResultEnvelope, UntrustedToolRequest,
    ValidatedToolRequest, WorkspaceResolution, build_agent_system_instructions,
};

fn budgets() -> ExecutionBudgets {
    ExecutionBudgets {
        rounds: 3,
        tool_calls: 6,
        elapsed_ms: 30_000,
        tool_ms: 10_000,
        context_tokens: 4_000,
        final_response_tokens: 500,
        tool_output_bytes: 32_000,
    }
}

fn ledger(run_id: &str) -> Result<ExecutionLedger, crate::ExecutionLedgerError> {
    let mut ledger = ExecutionLedger::new(
        run_id,
        LedgerRequest::new("request-1", "conversation-1", "project-1", "Run the task")?,
        build_agent_system_instructions("2026-08-17", None)
            .map_err(|error| crate::ExecutionLedgerError::InvalidRecord(error.to_string()))?
            .identity()
            .clone(),
        budgets(),
        LedgerNextAction::Generate,
    )?;
    ledger.resolve_workspace(WorkspaceResolution::new(
        "primary",
        "workspace-grant-1",
        "workspace-revision-1",
        LedgerCapability::ReadWrite,
    )?)?;
    Ok(ledger)
}

fn registry() -> Result<ToolRegistry, ToolRegistryError> {
    let definition = |name, classification, capability| {
        ToolDefinition::new(
            name,
            "Operate on one bounded file.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "workspace": { "type": "string", "enum": ["primary"] },
                    "path": { "type": "string", "minLength": 1 }
                },
                "required": ["workspace", "path"],
                "additionalProperties": false
            }),
            classification,
            capability,
            ApprovalRequirement::CapabilityOnly,
            RiskLevel::Low,
            ToolLimits::new(Duration::from_secs(5), 4_096, 1)?,
            [SupportedResultType::Text],
        )
    };
    ToolRegistry::new([
        definition(
            "filesystem.read_text",
            ToolClassification::Read,
            ProjectCapability::WorkspaceRead,
        )?,
        definition(
            "filesystem.write_text",
            ToolClassification::Write,
            ProjectCapability::WorkspaceWrite,
        )?,
    ])
}

fn request(
    registry: &ToolRegistry,
    name: &str,
    call_id: &str,
    path: &str,
) -> Result<ValidatedToolRequest, ToolRegistryError> {
    registry.validate_request(UntrustedToolRequest {
        call_id: call_id.to_owned(),
        name: name.to_owned(),
        arguments: serde_json::json!({"workspace": "primary", "path": path}),
    })
}

fn binding(
    request: &ValidatedToolRequest,
    scope_id: &str,
    path: &str,
) -> Result<PermissionBinding, crate::ExecutionLedgerError> {
    PermissionBinding::from_request(
        request,
        PermissionResourceScope::new(
            "project-1",
            PermissionResourceKind::Workspace,
            scope_id,
            "workspace-revision-1",
            vec![path.to_owned()],
        )?,
    )
}

fn completed(
    registry: &ToolRegistry,
    request: &ValidatedToolRequest,
) -> Result<ToolResultEnvelope, ToolRegistryError> {
    registry.completed(
        request,
        vec![ResultContent::Text {
            text: "done".to_owned(),
        }],
    )
}

fn record_mutation(
    ledger: &mut ExecutionLedger,
    request: &ValidatedToolRequest,
    result: &ToolResultEnvelope,
) -> Result<(), crate::ExecutionLedgerError> {
    ledger.record_mutation(
        request,
        result,
        "result-write",
        "mutation-1",
        vec![NewMutationChange {
            workspace_alias: "primary".to_owned(),
            relative_path: "src/lib.rs".to_owned(),
            previous_sha256: Some("a".repeat(64)),
            resulting_sha256: "b".repeat(64),
            resource_revision: "resource-r2".to_owned(),
        }],
    )
}

#[test]
fn every_round_receives_remaining_budgets_and_exhaustion_recovers_to_final_answer()
-> Result<(), Box<dyn std::error::Error>> {
    let mut ledger = ledger("run-budget")?;
    let mut controller = AgentLoopController::new(LoopLimits::new(4, 2)?);
    let LoopDirective::Generate(first) = controller.begin_round(&mut ledger) else {
        return Err("generation round missing".into());
    };
    assert_eq!(first.remaining.rounds, 2);
    controller.record_generation_usage(
        &mut ledger,
        GenerationUsage {
            generated_tokens: 4_000,
            elapsed_ms: 10,
        },
    )?;
    assert_eq!(
        controller.begin_round(&mut ledger),
        LoopDirective::PrepareFinalAnswer(FinalAnswerReason::BudgetExhausted)
    );
    assert!(!controller.record_final_response_usage(&mut ledger, 501, 10)?);
    assert_eq!(
        controller.finish(&mut ledger, false, false),
        LedgerTerminalState::NoChangeApplied
    );
    Ok(())
}

#[test]
fn duplicate_read_redirects_only_to_a_still_valid_ledger_observation()
-> Result<(), Box<dyn std::error::Error>> {
    let registry = registry()?;
    let first = request(
        &registry,
        "filesystem.read_text",
        "call-read-1",
        "src/lib.rs",
    )?;
    let second = request(
        &registry,
        "filesystem.read_text",
        "call-read-2",
        "src/lib.rs",
    )?;
    let first_binding = binding(&first, "workspace-grant-1", "src/lib.rs")?;
    let second_binding = binding(&second, "workspace-grant-1", "src/lib.rs")?;
    let result = completed(&registry, &first)?;
    let mut ledger = ledger("run-read")?;
    let mut controller = AgentLoopController::new(LoopLimits::new(4, 2)?);
    assert_eq!(
        controller.plan_tool_call(
            &mut ledger,
            &first,
            &first_binding,
            &ToolCallIntent::New,
            None,
        )?,
        ToolCallPlan::Execute
    );
    controller.record_tool_result(&mut ledger, &result, ToolExecutionUsage { elapsed_ms: 5 })?;
    ledger.record_observation(
        &first,
        &result,
        "result-read",
        NewObservation {
            fact_id: "observation-1".to_owned(),
            workspace_alias: "primary".to_owned(),
            relative_path: "src/lib.rs".to_owned(),
            resource_revision: "resource-r1".to_owned(),
            values: ObservedValues {
                content_sha256: Some("a".repeat(64)),
                ..ObservedValues::default()
            },
        },
    )?;
    let context = crate::ReuseContext {
        project_id: "project-1".to_owned(),
        workspace_alias: "primary".to_owned(),
        capability_id: "workspace-grant-1".to_owned(),
        capability_revision: "workspace-revision-1".to_owned(),
        resource_revision: "resource-r1".to_owned(),
    };
    let observation = ledger
        .reusable_observation("observation-1", &context)
        .cloned();
    assert!(matches!(
        controller.plan_tool_call(
            &mut ledger,
            &second,
            &second_binding,
            &ToolCallIntent::New,
            observation.as_ref(),
        )?,
        ToolCallPlan::ReuseObservation(fact) if fact.fact_id() == "observation-1"
    ));
    Ok(())
}

#[test]
fn retries_require_transient_exact_scope_and_stop_after_the_failure_limit()
-> Result<(), Box<dyn std::error::Error>> {
    let registry = registry()?;
    let first = request(&registry, "filesystem.read_text", "call-1", "src/lib.rs")?;
    let retry = request(&registry, "filesystem.read_text", "call-2", "src/lib.rs")?;
    let first_binding = binding(&first, "workspace-grant-1", "src/lib.rs")?;
    let retry_binding = binding(&retry, "workspace-grant-1", "src/lib.rs")?;
    let changed_scope = binding(&retry, "workspace-grant-2", "src/lib.rs")?;
    let mut ledger = ledger("run-retry")?;
    let mut controller = AgentLoopController::new(LoopLimits::new(4, 2)?);
    controller.plan_tool_call(
        &mut ledger,
        &first,
        &first_binding,
        &ToolCallIntent::New,
        None,
    )?;
    let failed = registry.failed(
        &first,
        ToolFailure::new("busy", "Resource is temporarily busy", true)?,
    )?;
    controller.record_tool_result(&mut ledger, &failed, ToolExecutionUsage { elapsed_ms: 5 })?;
    assert_eq!(
        controller.plan_tool_call(
            &mut ledger,
            &retry,
            &retry_binding,
            &ToolCallIntent::New,
            None,
        ),
        Err(LoopControlError::RetryRequired)
    );
    assert_eq!(
        controller.plan_tool_call(
            &mut ledger,
            &retry,
            &changed_scope,
            &ToolCallIntent::Retry {
                failed_call_id: "call-1".to_owned()
            },
            None,
        ),
        Err(LoopControlError::RetryScopeMismatch)
    );
    controller.plan_tool_call(
        &mut ledger,
        &retry,
        &retry_binding,
        &ToolCallIntent::Retry {
            failed_call_id: "call-1".to_owned(),
        },
        None,
    )?;
    let failed_again = registry.failed(
        &retry,
        ToolFailure::new("busy", "Resource is temporarily busy", true)?,
    )?;
    controller.record_tool_result(
        &mut ledger,
        &failed_again,
        ToolExecutionUsage { elapsed_ms: 5 },
    )?;
    assert_eq!(ledger.budgets().tool_calls, budgets().tool_calls - 2);
    assert_eq!(ledger.budgets().tool_ms, budgets().tool_ms - 10);
    assert_eq!(ledger.budgets().elapsed_ms, budgets().elapsed_ms - 10);
    assert!(ledger.budgets().tool_output_bytes < budgets().tool_output_bytes);
    assert_eq!(
        controller.begin_round(&mut ledger),
        LoopDirective::PrepareFinalAnswer(FinalAnswerReason::RepeatedFailure)
    );
    Ok(())
}

#[test]
fn successful_mutation_disables_tools_unless_an_unresolved_failure_exists()
-> Result<(), Box<dyn std::error::Error>> {
    let registry = registry()?;
    let write = request(
        &registry,
        "filesystem.write_text",
        "call-write",
        "src/lib.rs",
    )?;
    let result = completed(&registry, &write)?;
    let mut applied = ledger("run-applied")?;
    record_mutation(&mut applied, &write, &result)?;
    let mut controller = AgentLoopController::new(LoopLimits::new(4, 2)?);
    assert_eq!(
        controller.begin_round(&mut applied),
        LoopDirective::PrepareFinalAnswer(FinalAnswerReason::MutationApplied)
    );

    let mut unresolved = ledger("run-unresolved")?;
    record_mutation(&mut unresolved, &write, &result)?;
    let failed = registry.failed(
        &write,
        ToolFailure::new("verify", "Verification remains unresolved", true)?,
    )?;
    unresolved.record_unresolved_failure(
        &write,
        &failed,
        "result-failed",
        "failure-1",
        "verify",
        "Verification remains unresolved",
    )?;
    let mut controller = AgentLoopController::new(LoopLimits::new(4, 2)?);
    assert!(matches!(
        controller.begin_round(&mut unresolved),
        LoopDirective::Generate(_)
    ));
    Ok(())
}

#[test]
fn aggregate_tool_budgets_and_consecutive_rounds_force_bounded_recovery()
-> Result<(), Box<dyn std::error::Error>> {
    let registry = registry()?;
    let read = request(&registry, "filesystem.read_text", "call-read", "src/lib.rs")?;
    let read_binding = binding(&read, "workspace-grant-1", "src/lib.rs")?;
    let result = completed(&registry, &read)?;
    let mut ledger = ledger("run-tool-budget")?;
    let mut constrained = ledger.budgets();
    constrained.tool_output_bytes = 1;
    constrained.tool_ms = 1;
    ledger.set_budgets(constrained);
    let mut controller = AgentLoopController::new(LoopLimits::new(1, 2)?);
    controller.plan_tool_call(
        &mut ledger,
        &read,
        &read_binding,
        &ToolCallIntent::New,
        None,
    )?;
    controller.record_tool_result(&mut ledger, &result, ToolExecutionUsage { elapsed_ms: 2 })?;
    assert_eq!(
        controller.begin_round(&mut ledger),
        LoopDirective::PrepareFinalAnswer(FinalAnswerReason::BudgetExhausted)
    );

    let snapshot: AgentLoopSnapshot =
        serde_json::from_value(serde_json::to_value(controller.snapshot())?)?;
    assert_eq!(
        AgentLoopController::restore(snapshot)?.begin_round(&mut ledger),
        LoopDirective::PrepareFinalAnswer(FinalAnswerReason::BudgetExhausted)
    );
    Ok(())
}

#[test]
fn terminal_statuses_and_cancellation_are_host_derived() -> Result<(), Box<dyn std::error::Error>> {
    let registry = registry()?;
    let write = request(
        &registry,
        "filesystem.write_text",
        "call-write",
        "src/lib.rs",
    )?;
    let result = completed(&registry, &write)?;

    let mut complete = ledger("run-complete")?;
    let mut controller = AgentLoopController::new(LoopLimits::new(4, 2)?);
    assert_eq!(
        controller.finish(&mut complete, true, true),
        LedgerTerminalState::CompleteSuccess
    );

    let mut applied = ledger("run-incomplete")?;
    record_mutation(&mut applied, &write, &result)?;
    let mut controller = AgentLoopController::new(LoopLimits::new(4, 2)?);
    assert_eq!(
        controller.finish(&mut applied, true, false),
        LedgerTerminalState::ChangeAppliedResponseIncomplete
    );

    let mut partial = ledger("run-partial")?;
    let failed = registry.failed(
        &write,
        ToolFailure::new("verify", "Verification remains unresolved", true)?,
    )?;
    partial.record_unresolved_failure(
        &write,
        &failed,
        "result-failed",
        "failure-1",
        "verify",
        "Verification remains unresolved",
    )?;
    let mut controller = AgentLoopController::new(LoopLimits::new(4, 2)?);
    assert_eq!(
        controller.finish(&mut partial, false, true),
        LedgerTerminalState::PartialOutcome
    );

    let mut cancelled = ledger("run-cancelled")?;
    let mut controller = AgentLoopController::new(LoopLimits::new(4, 2)?);
    let pending = request(
        &registry,
        "filesystem.read_text",
        "call-pending",
        "src/lib.rs",
    )?;
    let pending_binding = binding(&pending, "workspace-grant-1", "src/lib.rs")?;
    controller.plan_tool_call(
        &mut cancelled,
        &pending,
        &pending_binding,
        &ToolCallIntent::New,
        None,
    )?;
    controller.cancel(&mut cancelled);
    assert_eq!(
        serde_json::to_value(controller.snapshot())?["active"],
        serde_json::json!({})
    );
    assert_eq!(
        controller.begin_round(&mut cancelled),
        LoopDirective::Terminal(LedgerTerminalState::Cancelled)
    );
    Ok(())
}
