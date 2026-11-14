use super::*;
use crate::{AppCore, ContractStepState};
use app_storage::{
    StoredDeclaredWorkflowStep, StoredWorkflowComparison, StoredWorkflowCondition,
    StoredWorkflowContract, StoredWorkflowDeclaration, StoredWorkflowRead,
};
use serde_json::json;
use std::sync::Arc;
fn declaration() -> StoredWorkflowDeclaration {
    let condition = StoredWorkflowCondition {
        contract: "app".into(),
        read: StoredWorkflowRead::SourceField {
            path: vec![json!("remaining")],
        },
        comparison: StoredWorkflowComparison::Equal,
        expected: json!("0"),
    };
    StoredWorkflowDeclaration {
        version: 2,
        network: "eip155:1".into(),
        account: "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd".into(),
        contracts: vec![StoredWorkflowContract {
            id: "app".into(),
            address: "0x2222222222222222222222222222222222222222".into(),
        }],
        steps: vec![StoredDeclaredWorkflowStep {
            id: "configure".into(),
            contract: "app".into(),
            function: "configure((uint256,string)[])".into(),
            arguments: vec![json!([["7", "a"]])],
            value_wei: "0".into(),
            depends_on: vec![],
            prerequisites: vec![],
            completion: vec![condition.clone()],
        }],
        goals: vec![condition],
    }
}
#[test]
fn generic_declaration_commits_full_typed_arguments_conditions_and_goals()
-> Result<(), Box<dyn std::error::Error>> {
    let original = declaration();
    let canonical = ContractWorkflowDeclaration::new(original.clone())?;
    assert_eq!(
        canonical.intent().steps[0].function,
        "configure((uint256,string)[])"
    );
    assert_eq!(canonical.condition_requests()?.len(), 2);
    let mut changed = original.clone();
    changed.goals[0].expected = json!("1");
    assert_ne!(
        canonical.commitment(),
        ContractWorkflowDeclaration::new(changed)?.commitment()
    );
    let mut changed = original.clone();
    changed.steps[0].completion[0].read = StoredWorkflowRead::SourceField {
        path: vec![json!("anotherField")],
    };
    assert_ne!(
        canonical.commitment(),
        ContractWorkflowDeclaration::new(changed)?.commitment()
    );
    let mut invalid = original;
    invalid.steps[0].arguments[0] = json!([[7, "a"]]);
    assert!(ContractWorkflowDeclaration::new(invalid).is_err());
    Ok(())
}
#[test]
fn abandoned_generic_claim_is_terminal_without_minting_another_current_turn_attempt()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let core = Arc::new(AppCore::initialize(temp.path())?);
    let project = core.create_project("Generic workflow", "2026-11-10T00:00:00Z")?;
    let conversation = core.create_project_conversation(&project.id, "2026-11-10T00:00:00Z")?;
    let conversation = core.append_user_message(
        &conversation.id,
        "configure source state".into(),
        vec![],
        "2026-11-10T00:00:01Z",
    )?;
    let message = conversation.messages.last().ok_or("message")?;
    let declaration = ContractWorkflowDeclaration::new(declaration())?;
    let session = core
        .start_declared_contract_workflow(&conversation.id, &message.id, declaration.clone())?
        .ok_or("session")?;
    let work = session.claim_next()?.ok_or("claim")?;
    assert_eq!(work.intent().function, "configure((uint256,string)[])");
    drop(work);
    assert_eq!(session.steps()?[0].state, ContractStepState::Failed);
    assert!(session.claim_next()?.is_none());
    assert!(
        core.start_declared_contract_workflow(&conversation.id, &message.id, declaration)?
            .is_none()
    );
    assert!(core.evm_transactions()?.is_empty());
    Ok(())
}
