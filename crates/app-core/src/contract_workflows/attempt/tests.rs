use super::*;
use app_storage::{
    StoredDeclaredWorkflowStep, StoredWorkflowComparison, StoredWorkflowCondition,
    StoredWorkflowContract, StoredWorkflowDeclaration, StoredWorkflowIntent, StoredWorkflowRead,
    StoredWorkflowScope,
};
use serde_json::json;
struct Fixture {
    directory: tempfile::TempDir,
    core: Arc<AppCore>,
    conversation: ConversationId,
    message: MessageId,
}
fn fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let core = Arc::new(AppCore::initialize(directory.path())?);
    let project = core.create_project("Workflow attempt", "2026-11-11T00:00:00Z")?;
    let conversation = core.create_project_conversation(&project.id, "2026-11-11T00:00:00Z")?;
    let conversation = core.append_user_message(
        &conversation.id,
        "configure contract".into(),
        vec![],
        "2026-11-11T00:00:01Z",
    )?;
    let message = conversation
        .messages
        .last()
        .ok_or("missing turn")?
        .id
        .clone();
    Ok(Fixture {
        directory,
        core,
        conversation: conversation.id,
        message,
    })
}
fn profile() -> Result<ContractWorkflowScope, app_storage::StorageError> {
    ContractWorkflowScope::new(StoredWorkflowScope {
        version: 1,
        network: "eip155:1".into(),
        account: format!("0x{}", "11".repeat(20)),
        steps: vec![StoredWorkflowIntent {
            target: format!("0x{}", "22".repeat(20)),
            profile_id: "fixture-v1".into(),
            profile_revision: 1,
            profile_commitment: "a".repeat(64),
            operation: "approve(address,uint256)".into(),
            arguments: vec![json!(format!("0x{}", "33".repeat(20))), json!("7")],
            value_wei: "0".into(),
        }],
    })
}
fn declared() -> Result<ContractWorkflowDeclaration, app_storage::StorageError> {
    let condition = StoredWorkflowCondition {
        contract: "app".into(),
        read: StoredWorkflowRead::SourceField {
            path: vec![json!("remaining")],
        },
        comparison: StoredWorkflowComparison::Equal,
        expected: json!("7"),
    };
    ContractWorkflowDeclaration::new(StoredWorkflowDeclaration {
        version: 2,
        network: "eip155:1".into(),
        account: format!("0x{}", "11".repeat(20)),
        contracts: vec![StoredWorkflowContract {
            id: "app".into(),
            address: format!("0x{}", "22".repeat(20)),
        }],
        steps: vec![StoredDeclaredWorkflowStep {
            id: "configure".into(),
            contract: "app".into(),
            function: "configure(uint256)".into(),
            arguments: vec![json!("7")],
            value_wei: "0".into(),
            depends_on: vec![],
            prerequisites: vec![],
            completion: vec![condition.clone()],
        }],
        goals: vec![condition],
    })
}
fn claim(fixture: &Fixture) -> Result<ContractWorkflowAttempt, Box<dyn std::error::Error>> {
    fixture
        .core
        .claim_contract_workflow_attempt(&fixture.conversation, &fixture.message)?
        .ok_or_else(|| "attempt missing".into())
}
fn blocked(fixture: &Fixture) -> Result<(), Box<dyn std::error::Error>> {
    assert!(
        fixture
            .core
            .claim_contract_workflow_attempt(&fixture.conversation, &fixture.message)?
            .is_none()
    );
    assert!(
        fixture
            .core
            .start_contract_workflow(&fixture.conversation, &fixture.message, profile()?)?
            .is_none()
    );
    assert!(
        fixture
            .core
            .start_declared_contract_workflow(&fixture.conversation, &fixture.message, declared()?)?
            .is_none()
    );
    assert!(!fixture.core.with_storage(|storage| {
        storage.claim_chat_transfer(&fixture.conversation, &fixture.message)
    })?);
    assert!(fixture.core.evm_transactions()?.is_empty());
    Ok(())
}
#[test]
fn account_free_reservation_starts_exact_profile_without_double_claim()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = fixture()?;
    let attempt = claim(&fixture)?; // No selected account/wallet scope exists yet.
    blocked(&fixture)?;
    assert!(
        fixture
            .core
            .with_storage(|storage| storage
                .contract_workflow_for_turn(&fixture.conversation, &fixture.message))?
            .is_none()
    );
    let scope = profile()?;
    let session = attempt.start_profile(scope.clone())?.ok_or("session")?;
    assert_eq!(session.scope(), &scope);
    assert_eq!(session.steps()?.len(), 1);
    blocked(&fixture)?;
    Ok(())
}
#[test]
fn same_account_free_reservation_can_instead_start_exact_declared_scope()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = fixture()?;
    let attempt = claim(&fixture)?;
    let declaration = declared()?;
    let session = attempt
        .start_declared(declaration.clone())?
        .ok_or("session")?;
    assert_eq!(session.declaration(), &declaration);
    assert_eq!(session.steps()?.len(), 1);
    blocked(&fixture)?;
    Ok(())
}
#[test]
fn cancellation_drop_and_restart_leave_shared_attempt_consumed()
-> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = fixture()?;
    drop(claim(&fixture)?);
    blocked(&fixture)?;
    drop(fixture.core);
    fixture.core = Arc::new(AppCore::initialize(fixture.directory.path())?);
    blocked(&fixture)?;
    assert!(
        fixture
            .core
            .with_storage(|storage| storage
                .contract_workflow_for_turn(&fixture.conversation, &fixture.message))?
            .is_none()
    );
    Ok(())
}
#[test]
fn user_turn_changed_during_unlock_cannot_attach_either_version()
-> Result<(), Box<dyn std::error::Error>> {
    for declared_scope in [false, true] {
        let fixture = fixture()?;
        let attempt = claim(&fixture)?;
        fixture.core.append_user_message(
            &fixture.conversation,
            "new request".into(),
            vec![],
            "2026-11-11T00:00:02Z",
        )?;
        if declared_scope {
            assert!(attempt.start_declared(declared()?)?.is_none());
        } else {
            assert!(attempt.start_profile(profile()?)?.is_none());
        }
        blocked(&fixture)?;
        assert!(
            fixture
                .core
                .with_storage(|storage| storage
                    .contract_workflow_for_turn(&fixture.conversation, &fixture.message))?
                .is_none()
        );
    }
    Ok(())
}
#[test]
fn repeated_pre_prompt_requests_have_one_opaque_owner() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = fixture()?;
    let barrier = std::sync::Barrier::new(2);
    let results = std::thread::scope(|scope| {
        let request = || {
            barrier.wait();
            fixture
                .core
                .claim_contract_workflow_attempt(&fixture.conversation, &fixture.message)
                .map(|attempt| attempt.is_some())
        };
        let left = scope.spawn(request);
        let right = scope.spawn(request);
        (left.join(), right.join())
    });
    assert_ne!(
        results.0.map_err(|_| "left panicked")??,
        results.1.map_err(|_| "right panicked")??
    );
    blocked(&fixture)?;
    Ok(())
}
