use std::time::Duration;

use super::*;
use crate::{
    AGENT_SYSTEM_POLICY_VERSION, ApprovalRequirement, LedgerTerminalState, ObservedValues,
    PermissionResourceKind, PermissionResourceScope, ProjectCapability, ResultContent, RiskLevel,
    SupportedResultType, ToolDefinition, ToolFailure, ToolLimits, ToolRegistry, ToolRegistryError,
    UntrustedToolRequest, build_agent_system_instructions,
};

fn budgets() -> ExecutionBudgets {
    ExecutionBudgets {
        rounds: 5,
        tool_calls: 12,
        elapsed_ms: 60_000,
        tool_ms: 20_000,
        context_tokens: 8_000,
        final_response_tokens: 1_000,
        tool_output_bytes: 64_000,
    }
}

fn registry() -> Result<ToolRegistry, ToolRegistryError> {
    let definition = |name, classification, capability| {
        ToolDefinition::new(
            name,
            "Operate on one bounded project file.",
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
            ToolLimits::new(Duration::from_secs(5), 4_096, 2)?,
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
) -> Result<ValidatedToolRequest, ToolRegistryError> {
    registry.validate_request(UntrustedToolRequest {
        call_id: call_id.to_owned(),
        name: name.to_owned(),
        arguments: serde_json::json!({"workspace": "primary", "path": "src/lib.rs"}),
    })
}

fn ledger() -> Result<ExecutionLedger, ExecutionLedgerError> {
    let mut ledger = ExecutionLedger::new(
        "run-1",
        LedgerRequest::new(
            "request-1",
            "conversation-1",
            "project-1",
            "Update src/lib.rs safely",
        )?,
        build_agent_system_instructions("2026-08-17", None)
            .map_err(|error| ExecutionLedgerError::InvalidRecord(error.to_string()))?
            .identity()
            .clone(),
        budgets(),
        LedgerNextAction::Generate,
    )?;
    ledger.resolve_workspace(WorkspaceResolution::new(
        "primary",
        "secret-capability-handle",
        "secret-capability-revision-1",
        LedgerCapability::ReadWrite,
    )?)?;
    Ok(ledger)
}

#[test]
fn observations_reuse_only_under_exact_live_assumptions() -> Result<(), Box<dyn std::error::Error>>
{
    let registry = registry()?;
    let read = request(&registry, "filesystem.read_text", "call-read")?;
    let read_result = registry.completed(
        &read,
        vec![ResultContent::Text {
            text: "file content".to_owned(),
        }],
    )?;
    let mut ledger = ledger()?;
    ledger.record_observation(
        &read,
        &read_result,
        "result-read",
        NewObservation {
            fact_id: "fact-read".to_owned(),
            workspace_alias: "primary".to_owned(),
            relative_path: "src/lib.rs".to_owned(),
            resource_revision: "resource-r1".to_owned(),
            values: ObservedValues {
                content_sha256: Some("a".repeat(64)),
                match_count: Some(3),
                byte_size: Some(128),
                line_count: Some(8),
            },
        },
    )?;
    let exact = ReuseContext {
        project_id: "project-1".to_owned(),
        workspace_alias: "primary".to_owned(),
        capability_id: "secret-capability-handle".to_owned(),
        capability_revision: "secret-capability-revision-1".to_owned(),
        resource_revision: "resource-r1".to_owned(),
    };
    assert!(ledger.reusable_observation("fact-read", &exact).is_some());

    let mut stale = exact.clone();
    stale.resource_revision = "resource-r2".to_owned();
    assert!(ledger.reusable_observation("fact-read", &stale).is_none());
    stale = exact.clone();
    stale.capability_revision = "secret-capability-revision-2".to_owned();
    assert!(ledger.reusable_observation("fact-read", &stale).is_none());
    stale = exact.clone();
    stale.project_id = "project-2".to_owned();
    assert!(ledger.reusable_observation("fact-read", &stale).is_none());
    Ok(())
}

#[test]
fn successful_mutation_remains_authoritative_after_restore()
-> Result<(), Box<dyn std::error::Error>> {
    let registry = registry()?;
    let read = request(&registry, "filesystem.read_text", "call-read")?;
    let read_result = registry.completed(
        &read,
        vec![ResultContent::Text {
            text: "old".to_owned(),
        }],
    )?;
    let write = request(&registry, "filesystem.write_text", "call-write")?;
    let write_result = registry.completed(
        &write,
        vec![ResultContent::Text {
            text: "applied".to_owned(),
        }],
    )?;
    let mut ledger = ledger()?;
    ledger.record_observation(
        &read,
        &read_result,
        "result-read",
        NewObservation {
            fact_id: "fact-read".to_owned(),
            workspace_alias: "primary".to_owned(),
            relative_path: "src/lib.rs".to_owned(),
            resource_revision: "resource-r1".to_owned(),
            values: ObservedValues {
                content_sha256: Some("a".repeat(64)),
                ..ObservedValues::default()
            },
        },
    )?;
    ledger.record_mutation(
        &write,
        &write_result,
        "result-write",
        "mutation-1",
        vec![NewMutationChange {
            workspace_alias: "primary".to_owned(),
            relative_path: "src/lib.rs".to_owned(),
            previous_sha256: Some("a".repeat(64)),
            resulting_sha256: "b".repeat(64),
            resource_revision: "resource-r2".to_owned(),
        }],
    )?;
    let context = ReuseContext {
        project_id: "project-1".to_owned(),
        workspace_alias: "primary".to_owned(),
        capability_id: "secret-capability-handle".to_owned(),
        capability_revision: "secret-capability-revision-1".to_owned(),
        resource_revision: "resource-r1".to_owned(),
    };
    assert!(ledger.reusable_observation("fact-read", &context).is_none());

    let snapshot: ExecutionLedgerSnapshot =
        serde_json::from_value(serde_json::to_value(ledger.snapshot()?)?)?;
    let restored = ExecutionLedger::restore(snapshot)?;
    assert_eq!(
        restored.system_instructions().policy_version(),
        AGENT_SYSTEM_POLICY_VERSION
    );
    assert_eq!(
        restored.system_instructions().content_sha256(),
        ledger.system_instructions().content_sha256()
    );
    let authoritative = restored
        .authoritative_mutation("primary", "src/lib.rs")
        .ok_or("authoritative mutation missing")?;
    assert_eq!(authoritative.resulting_sha256(), "b".repeat(64));
    assert_eq!(authoritative.resource_revision(), "resource-r2");
    Ok(())
}

#[test]
fn projection_omits_authority_and_model_arguments() -> Result<(), Box<dyn std::error::Error>> {
    let registry = registry()?;
    let read = request(&registry, "filesystem.read_text", "call-read")?;
    let result = registry.completed(
        &read,
        vec![ResultContent::Text {
            text: "safe".to_owned(),
        }],
    )?;
    let mut ledger = ledger()?;
    let binding = PermissionBinding::from_request(
        &read,
        PermissionResourceScope::new(
            "project-1",
            PermissionResourceKind::Workspace,
            "secret-capability-handle",
            "secret-capability-revision-1",
            vec!["src/lib.rs".to_owned()],
        )?,
    )?;
    ledger.record_permission(
        &read,
        &binding,
        "permission-fact-1",
        "permission-1",
        LedgerPermissionDecision::Pending,
    )?;
    ledger.record_permission(
        &read,
        &binding,
        "permission-fact-2",
        "permission-1",
        LedgerPermissionDecision::Approved,
    )?;
    ledger.record_observation(
        &read,
        &result,
        "result-read",
        NewObservation {
            fact_id: "fact-read".to_owned(),
            workspace_alias: "primary".to_owned(),
            relative_path: "src/lib.rs".to_owned(),
            resource_revision: "resource-r1".to_owned(),
            values: ObservedValues {
                match_count: Some(1),
                ..ObservedValues::default()
            },
        },
    )?;
    let encoded = serde_json::to_string(&ledger.model_projection())?;
    assert!(encoded.contains("primary"));
    assert!(encoded.contains("src/lib.rs"));
    assert!(encoded.contains("permission-1"));
    assert!(!encoded.contains("secret-capability-handle"));
    assert!(!encoded.contains("secret-capability-revision-1"));
    assert!(!encoded.contains("capabilityId"));
    assert!(!encoded.contains("canonicalRoot"));
    assert!(!encoded.contains("bookmark"));
    assert!(!encoded.contains("arguments"));
    Ok(())
}

#[test]
fn failures_partial_outcomes_budgets_and_next_action_are_trusted_facts()
-> Result<(), Box<dyn std::error::Error>> {
    let registry = registry()?;
    let read = request(&registry, "filesystem.read_text", "call-read")?;
    let failure = ToolFailure::new("changed", "The file changed during the read", true)?;
    let failed_result = registry.failed(&read, failure)?;
    let completed_result = registry.completed(
        &read,
        vec![ResultContent::Text {
            text: "partial".to_owned(),
        }],
    )?;
    let mut ledger = ledger()?;
    ledger.record_unresolved_failure(
        &read,
        &failed_result,
        "result-failed",
        "failure-1",
        "changed",
        "The file changed during the read",
    )?;
    ledger.record_partial_outcome(
        &read,
        &completed_result,
        "result-partial",
        "partial-1",
        "One bounded result still needs follow-up",
    )?;
    let remaining = ExecutionBudgets {
        rounds: 1,
        tool_calls: 2,
        ..budgets()
    };
    ledger.set_budgets(remaining);
    ledger.set_next_action(LedgerNextAction::Terminal {
        state: LedgerTerminalState::PartialOutcome,
    })?;
    let projection = ledger.model_projection();
    let encoded = serde_json::to_string(&projection)?;
    assert!(encoded.contains("failure-1"));
    assert!(encoded.contains("partial-1"));
    assert!(encoded.contains("partial"));
    assert_eq!(ledger.budgets(), remaining);
    Ok(())
}

#[test]
fn malformed_scope_and_duplicate_fact_ids_fail_closed() -> Result<(), Box<dyn std::error::Error>> {
    let registry = registry()?;
    let read = request(&registry, "filesystem.read_text", "call-read")?;
    let result = registry.completed(
        &read,
        vec![ResultContent::Text {
            text: "safe".to_owned(),
        }],
    )?;
    let mut ledger = ledger()?;
    let observation = || NewObservation {
        fact_id: "fact-1".to_owned(),
        workspace_alias: "primary".to_owned(),
        relative_path: "src/lib.rs".to_owned(),
        resource_revision: "resource-r1".to_owned(),
        values: ObservedValues {
            byte_size: Some(10),
            ..ObservedValues::default()
        },
    };
    ledger.record_observation(&read, &result, "result-1", observation())?;
    assert_eq!(
        ledger.record_observation(&read, &result, "result-2", observation()),
        Err(ExecutionLedgerError::DuplicateFact("fact-1".to_owned()))
    );

    let mut tampered = serde_json::to_value(ledger.snapshot()?)?;
    tampered["observations"][0]["origin"]["projectId"] = serde_json::json!("project-other");
    let tampered: ExecutionLedgerSnapshot = serde_json::from_value(tampered)?;
    assert_eq!(
        ExecutionLedger::restore(tampered),
        Err(ExecutionLedgerError::ScopeMismatch)
    );

    let mut tampered = serde_json::to_value(ledger.snapshot()?)?;
    tampered["systemInstructions"]["policyVersion"] = serde_json::json!(999);
    let tampered: ExecutionLedgerSnapshot = serde_json::from_value(tampered)?;
    assert!(matches!(
        ExecutionLedger::restore(tampered),
        Err(ExecutionLedgerError::InvalidRecord(_))
    ));
    Ok(())
}
