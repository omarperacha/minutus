use std::time::Duration;

use super::*;
use crate::{
    AgentStateKind, AgentStateMachine, AgentStateMachineError, ApprovalRequirement,
    LedgerPermissionDecision, ProjectCapability, RiskLevel, SupportedResultType,
    ToolClassification, ToolDefinition, ToolLimits, ToolRegistry, UntrustedToolRequest,
};

fn registry() -> Result<ToolRegistry, Box<dyn std::error::Error>> {
    let definition = |name, classification, capability, approval| {
        ToolDefinition::new(
            name,
            "Operate on one project file.",
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
            approval,
            RiskLevel::Low,
            ToolLimits::new(Duration::from_secs(5), 2_048, 1)?,
            [SupportedResultType::Text],
        )
    };
    Ok(ToolRegistry::new([
        definition(
            "computation.run",
            ToolClassification::Read,
            ProjectCapability::NoProjectAuthority,
            ApprovalRequirement::CapabilityOnly,
        )?,
        definition(
            "filesystem.read_text",
            ToolClassification::Read,
            ProjectCapability::WorkspaceRead,
            ApprovalRequirement::CapabilityOnly,
        )?,
        definition(
            "filesystem.write_text",
            ToolClassification::Write,
            ProjectCapability::WorkspaceWrite,
            ApprovalRequirement::CapabilityOnly,
        )?,
        definition(
            "external.publish",
            ToolClassification::ExternalSideEffect,
            ProjectCapability::ExternalNetwork,
            ApprovalRequirement::Interactive,
        )?,
    ])?)
}

#[test]
fn explicit_no_authority_tool_neither_invents_nor_accepts_a_durable_capability()
-> Result<(), Box<dyn std::error::Error>> {
    let registry = registry()?;
    let request = request(&registry, "computation.run", "call-pure", "pure")?;
    let mediator = PermissionMediator::default();
    let project_scope = scope(PermissionResourceKind::Project, "pure-computation")?;
    let authorization = mediator.authorize_without_capability(&request, project_scope.clone())?;
    assert_eq!(
        authorization.binding().capability(),
        ProjectCapability::NoProjectAuthority
    );
    assert!(
        DurableCapability::new(
            "project-1",
            ProjectCapability::NoProjectAuthority,
            "scope-secret-1",
            "scope-revision-1",
        )
        .is_err()
    );
    Ok(())
}

fn request(
    registry: &ToolRegistry,
    name: &str,
    call_id: &str,
    path: &str,
) -> Result<crate::ValidatedToolRequest, crate::ToolRegistryError> {
    registry.validate_request(UntrustedToolRequest {
        call_id: call_id.to_owned(),
        name: name.to_owned(),
        arguments: serde_json::json!({"workspace": "primary", "path": path}),
    })
}

fn scope(
    kind: PermissionResourceKind,
    resource: &str,
) -> Result<PermissionResourceScope, crate::ExecutionLedgerError> {
    PermissionResourceScope::new(
        "project-1",
        kind,
        "scope-secret-1",
        "scope-revision-1",
        vec![resource.to_owned()],
    )
}

#[test]
fn durable_workspace_capabilities_authorize_ordinary_reads_and_granted_writes()
-> Result<(), Box<dyn std::error::Error>> {
    let registry = registry()?;
    let mediator = PermissionMediator::default();
    for (name, capability) in [
        ("filesystem.read_text", ProjectCapability::WorkspaceRead),
        ("filesystem.write_text", ProjectCapability::WorkspaceWrite),
    ] {
        let request = request(&registry, name, "call-1", "src/lib.rs")?;
        let grant = DurableCapability::new(
            "project-1",
            capability,
            "scope-secret-1",
            "scope-revision-1",
        )?;
        let authorization = mediator.authorize_capability(
            &request,
            scope(PermissionResourceKind::Workspace, "src/lib.rs")?,
            &grant,
        )?;
        assert_eq!(authorization.binding().call_id(), "call-1");
    }

    let request = request(
        &registry,
        "filesystem.read_text",
        "call-stale",
        "src/lib.rs",
    )?;
    let stale = DurableCapability::new(
        "project-1",
        ProjectCapability::WorkspaceRead,
        "scope-secret-1",
        "scope-revision-old",
    )?;
    assert_eq!(
        mediator.authorize_capability(
            &request,
            scope(PermissionResourceKind::Workspace, "src/lib.rs")?,
            &stale,
        ),
        Err(PermissionMediationError::CapabilityNotGranted)
    );
    Ok(())
}

#[test]
fn interactive_decision_is_exact_and_cannot_be_modified_or_replayed()
-> Result<(), Box<dyn std::error::Error>> {
    let registry = registry()?;
    let original = request(&registry, "external.publish", "call-publish", "draft.md")?;
    let modified = request(&registry, "external.publish", "call-publish", "secret.md")?;
    let resource_scope = scope(PermissionResourceKind::External, "draft.md")?;
    let mut mediator = PermissionMediator::default();
    mediator.request_interactive("permission-1", &original, resource_scope.clone())?;

    assert_eq!(
        mediator.decide(
            "permission-1",
            &modified,
            &resource_scope,
            LedgerPermissionDecision::Approved,
        ),
        Err(PermissionMediationError::BindingMismatch)
    );
    let receipt = mediator.decide(
        "permission-1",
        &original,
        &resource_scope,
        LedgerPermissionDecision::Approved,
    )?;
    assert!(receipt.authorization.is_some());
    assert_eq!(
        mediator.decide(
            "permission-1",
            &original,
            &resource_scope,
            LedgerPermissionDecision::Approved,
        ),
        Err(PermissionMediationError::StaleOrReplayed)
    );
    Ok(())
}

#[test]
fn non_approval_decisions_are_distinct_terminal_outcomes_without_authority()
-> Result<(), Box<dyn std::error::Error>> {
    let registry = registry()?;
    let request = request(&registry, "external.publish", "call-publish", "draft.md")?;
    for (index, decision) in [
        LedgerPermissionDecision::Denied,
        LedgerPermissionDecision::Dismissed,
        LedgerPermissionDecision::Expired,
        LedgerPermissionDecision::Cancelled,
    ]
    .into_iter()
    .enumerate()
    {
        let mut mediator = PermissionMediator::default();
        let permission_id = format!("permission-{index}");
        let resource_scope = scope(PermissionResourceKind::External, "draft.md")?;
        mediator.request_interactive(&permission_id, &request, resource_scope.clone())?;
        let receipt = mediator.decide(&permission_id, &request, &resource_scope, decision)?;
        assert_eq!(receipt.decision, decision);
        assert!(receipt.authorization.is_none());
    }
    Ok(())
}

#[test]
fn state_machine_requires_matching_mediator_authorization() -> Result<(), Box<dyn std::error::Error>>
{
    let registry = registry()?;
    let pending = request(&registry, "external.publish", "call-publish", "draft.md")?;
    let other = request(&registry, "external.publish", "call-other", "draft.md")?;
    let resource_scope = scope(PermissionResourceKind::External, "draft.md")?;
    let mut mediator = PermissionMediator::default();
    mediator.request_interactive("permission-other", &other, resource_scope.clone())?;
    let wrong = mediator
        .decide(
            "permission-other",
            &other,
            &resource_scope,
            LedgerPermissionDecision::Approved,
        )?
        .authorization
        .ok_or("authorization missing")?;

    let mut machine = AgentStateMachine::start("run-1", "generation-1")?;
    let token = machine.request_tool(&machine.transition_token(), &pending)?;
    let token = machine.await_permission(&token)?;
    assert_eq!(
        machine.approve_and_start_tool(&token, wrong, true),
        Err(AgentStateMachineError::MismatchedAuthorization)
    );
    assert_eq!(machine.state().kind(), AgentStateKind::AwaitingPermission);
    Ok(())
}

#[test]
fn restart_state_is_validated_and_ui_prompt_omits_authority_data()
-> Result<(), Box<dyn std::error::Error>> {
    let registry = registry()?;
    let request = request(&registry, "external.publish", "call-publish", "draft.md")?;
    let resource_scope = scope(PermissionResourceKind::External, "draft.md")?;
    let mut mediator = PermissionMediator::default();
    let prompt = mediator.request_interactive("permission-1", &request, resource_scope)?;
    let prompt_json = serde_json::to_string(&prompt)?;
    assert!(prompt_json.contains("draft.md"));
    assert!(!prompt_json.contains("scope-secret-1"));
    assert!(!prompt_json.contains("argumentsSha256"));

    let snapshot: PermissionMediatorSnapshot =
        serde_json::from_value(serde_json::to_value(mediator.snapshot())?)?;
    let restored = PermissionMediator::restore(snapshot)?;
    assert_eq!(restored.prompt("permission-1"), Some(prompt));
    Ok(())
}
