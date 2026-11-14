use super::*;
use crate::AppCore;
use app_storage::{ContractStepState, StoredWorkflowIntent, StoredWorkflowScope};
use serde_json::json;
use std::sync::Arc;
const ACCOUNT: &str = "0x1111111111111111111111111111111111111111";
fn scope() -> StoredWorkflowScope {
    StoredWorkflowScope {
        version: 1,
        network: "eip155:8453".into(),
        account: ACCOUNT.into(),
        steps: vec![
            StoredWorkflowIntent {
                target: "0x2222222222222222222222222222222222222222".into(),
                profile_id: "fixture-v1".into(),
                profile_revision: 1,
                profile_commitment: "a".repeat(64),
                operation: "approve(address,uint256)".into(),
                arguments: vec![json!(ACCOUNT), json!("7")],
                value_wei: "0".into(),
            },
            StoredWorkflowIntent {
                target: "0x2222222222222222222222222222222222222222".into(),
                profile_id: "fixture-v1".into(),
                profile_revision: 1,
                profile_commitment: "a".repeat(64),
                operation: "transferFrom(address,address,uint256)".into(),
                arguments: vec![
                    json!(ACCOUNT),
                    json!("0x3333333333333333333333333333333333333333"),
                    json!("7"),
                ],
                value_wei: "0".into(),
            },
        ],
    }
}
#[test]
fn scope_rejects_untyped_or_mutated_values_and_commits_only_fixed_intent()
-> Result<(), Box<dyn std::error::Error>> {
    let original = scope();
    let scope = ContractWorkflowScope::new(original.clone())?;
    assert_eq!(scope, ContractWorkflowScope::new(original.clone())?);
    let mut changed = original.clone();
    changed.steps[0].arguments[1] = json!("8");
    assert_ne!(
        scope.commitment(),
        ContractWorkflowScope::new(changed)?.commitment()
    );
    let mut bad = original.clone();
    bad.steps[0].arguments[1] = json!(7);
    assert!(ContractWorkflowScope::new(bad).is_err());
    let mut bad = original;
    bad.steps[0].operation = "approve(address,uint)".into();
    assert!(ContractWorkflowScope::new(bad).is_err());
    Ok(())
}
#[test]
fn denial_stops_ordered_workflow_without_approval_or_automatic_cleanup()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let core = Arc::new(AppCore::initialize(temp.path())?);
    let project = core.create_project("Workflow", "2026-10-30T00:00:00Z")?;
    let conversation = core.create_project_conversation(&project.id, "2026-10-30T00:00:00Z")?;
    let conversation = core.append_user_message(
        &conversation.id,
        "approve then transfer".into(),
        vec![],
        "2026-10-30T00:00:01Z",
    )?;
    let message = conversation.messages.last().ok_or("message")?;
    let session = core
        .start_contract_workflow(
            &conversation.id,
            &message.id,
            ContractWorkflowScope::new(scope())?,
        )?
        .ok_or("workflow")?;
    let step = session.claim_next(vec![])?.ok_or("step")?;
    assert_eq!(step.index(), 0);
    assert!(step.stop_before_send(ContractStepState::Denied)?);
    assert!(session.claim_next(vec![])?.is_none());
    assert_eq!(session.steps()?[1].state, ContractStepState::Planned);
    assert!(core.evm_transactions()?.is_empty());
    // Stage aliases, fresh model call IDs and replanning are absent from the
    // durable identity. A changed plan cannot mint another current-turn attempt.
    let mut replanned = scope();
    replanned.steps[0].arguments[1] = json!("8");
    assert!(
        core.start_contract_workflow(
            &conversation.id,
            &message.id,
            ContractWorkflowScope::new(replanned)?
        )?
        .is_none()
    );
    Ok(())
}
