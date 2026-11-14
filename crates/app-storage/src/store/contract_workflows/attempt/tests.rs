use super::*;
use crate::store::contract_workflows::{
    StoredDeclaredWorkflowStep, StoredWorkflowComparison, StoredWorkflowCondition,
    StoredWorkflowContract, StoredWorkflowIntent, StoredWorkflowRead,
};
use serde_json::json;
fn setup(path: &std::path::Path) -> Result<ApplicationStore, StorageError> {
    let store = ApplicationStore::open(path)?;
    store.connection.execute(
        "INSERT INTO projects(id,name,created_at,updated_at) VALUES ('p','P','now','now')",
        [],
    )?;
    store.connection.execute("INSERT INTO conversations(id,title,created_at,updated_at,project_id) VALUES ('c','C','now','now','p')",[])?;
    store.connection.execute("INSERT INTO messages(id,conversation_id,position,role,parts_json,created_at) VALUES ('m','c',0,'user','[]','now')",[])?;
    Ok(store)
}
fn turn() -> (ConversationId, MessageId) {
    (ConversationId("c".into()), MessageId("m".into()))
}
fn profile() -> StoredWorkflowScope {
    StoredWorkflowScope {
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
    }
}
fn declared() -> StoredWorkflowDeclaration {
    let condition = StoredWorkflowCondition {
        contract: "app".into(),
        read: StoredWorkflowRead::SourceField {
            path: vec![json!("remaining")],
        },
        comparison: StoredWorkflowComparison::Equal,
        expected: json!("7"),
    };
    StoredWorkflowDeclaration {
        version: 2,
        network: "eip155:1".into(),
        account: format!("0x{}", "11".repeat(20)),
        contracts: vec![StoredWorkflowContract {
            id: "app".into(),
            address: format!("0x{}", "22".repeat(20)),
        }],
        steps: vec![StoredDeclaredWorkflowStep {
            id: "set".into(),
            contract: "app".into(),
            function: "configure(uint256)".into(),
            arguments: vec![json!("7")],
            value_wei: "0".into(),
            depends_on: vec![],
            prerequisites: vec![],
            completion: vec![condition.clone()],
        }],
        goals: vec![condition],
    }
}
fn reserve(
    store: &ApplicationStore,
) -> Result<ContractWorkflowAttemptClaim, Box<dyn std::error::Error>> {
    let (conversation, message) = turn();
    store
        .claim_contract_workflow_attempt(&conversation, &message)?
        .ok_or_else(|| "missing attempt".into())
}
fn no_workflow(store: &ApplicationStore) -> Result<(), StorageError> {
    let (conversation, message) = turn();
    assert!(
        store
            .contract_workflow_for_turn(&conversation, &message)?
            .is_none()
    );
    Ok(())
}
#[test]
fn reservation_blocks_transfer_and_both_versions_before_scope_exists()
-> Result<(), Box<dyn std::error::Error>> {
    for declared_scope in [false, true] {
        let directory = tempfile::tempdir()?;
        let mut store = setup(&directory.path().join("state.sqlite"))?;
        let attempt = reserve(&store)?;
        let (conversation, message) = turn();
        no_workflow(&store)?;
        assert!(!store.claim_chat_transfer(&conversation, &message)?);
        assert!(
            store
                .claim_contract_workflow_attempt(&conversation, &message)?
                .is_none()
        );
        assert!(
            store
                .create_scoped_contract_workflow(&conversation, &message, &profile())?
                .is_none()
        );
        assert!(
            store
                .create_declared_contract_workflow(&conversation, &message, &declared())?
                .is_none()
        );
        let id = if declared_scope {
            store.attach_declared_contract_workflow(attempt, &declared())?
        } else {
            store.attach_scoped_contract_workflow(attempt, &profile())?
        }
        .ok_or("manifest")?;
        if declared_scope {
            assert_eq!(store.contract_workflow_declaration(&id)?, Some(declared()));
        } else {
            assert_eq!(store.contract_workflow_scope(&id)?, Some(profile()));
        }
        assert_eq!(store.contract_workflow_steps(&id)?.len(), 1);
    }
    Ok(())
}
#[test]
fn dropped_claim_and_existing_standalone_claim_cannot_be_reconstructed_after_reopen()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("state.sqlite");
    let store = setup(&path)?;
    drop(reserve(&store)?);
    drop(store);
    let mut reopened = ApplicationStore::open(&path)?;
    let (conversation, message) = turn();
    assert!(
        reopened
            .claim_contract_workflow_attempt(&conversation, &message)?
            .is_none()
    );
    assert!(!reopened.claim_chat_transfer(&conversation, &message)?);
    assert!(
        reopened
            .create_scoped_contract_workflow(&conversation, &message, &profile())?
            .is_none()
    );
    assert!(
        reopened
            .create_declared_contract_workflow(&conversation, &message, &declared())?
            .is_none()
    );
    no_workflow(&reopened)?;
    let directory = tempfile::tempdir()?;
    let store = setup(&directory.path().join("standalone.sqlite"))?;
    assert!(store.claim_chat_transfer(&conversation, &message)?);
    assert!(
        store
            .claim_contract_workflow_attempt(&conversation, &message)?
            .is_none()
    );
    Ok(())
}
#[test]
fn claim_cannot_cross_database_handles_or_reopened_storage_owners()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("left.sqlite");
    let left = setup(&path)?;
    let attempt = reserve(&left)?;
    let mut reopened = ApplicationStore::open(&path)?;
    assert!(
        reopened
            .attach_scoped_contract_workflow(attempt, &profile())
            .is_err()
    );
    no_workflow(&reopened)?;
    let attempt = {
        let directory = tempfile::tempdir()?;
        let store = setup(&directory.path().join("other.sqlite"))?;
        reserve(&store)?
    };
    assert!(
        reopened
            .attach_declared_contract_workflow(attempt, &declared())
            .is_err()
    );
    no_workflow(&reopened)?;
    let (conversation, message) = turn();
    assert!(
        reopened
            .claim_contract_workflow_attempt(&conversation, &message)?
            .is_none()
    );
    Ok(())
}
#[test]
fn attachment_rechecks_latest_user_and_tombstone_and_never_installs_partial_scope()
-> Result<(), Box<dyn std::error::Error>> {
    for remove_tombstone in [false, true] {
        let directory = tempfile::tempdir()?;
        let mut store = setup(&directory.path().join("state.sqlite"))?;
        let attempt = reserve(&store)?;
        if remove_tombstone {
            store
                .connection
                .execute("DELETE FROM chat_transfer_attempts", [])?;
        } else {
            store.connection.execute("INSERT INTO messages(id,conversation_id,position,role,parts_json,created_at) VALUES ('new','c',1,'user','[]','now')",[])?;
        }
        assert!(
            store
                .attach_declared_contract_workflow(attempt, &declared())?
                .is_none()
        );
        no_workflow(&store)?;
    }
    Ok(())
}
#[test]
fn invalid_scope_and_failed_atomic_attach_do_not_refund_the_preclaimed_turn()
-> Result<(), Box<dyn std::error::Error>> {
    for invalid in [false, true] {
        let directory = tempfile::tempdir()?;
        let mut store = setup(&directory.path().join("state.sqlite"))?;
        let attempt = reserve(&store)?;
        let mut scope = profile();
        if invalid {
            scope.steps.clear();
        } else {
            store.connection.execute_batch("CREATE TEMP TRIGGER fail_scope BEFORE INSERT ON contract_workflow_scopes BEGIN SELECT RAISE(ABORT,'fixture failure'); END;")?;
        }
        assert!(
            store
                .attach_scoped_contract_workflow(attempt, &scope)
                .is_err()
        );
        no_workflow(&store)?;
        let (conversation, message) = turn();
        assert!(!store.claim_chat_transfer(&conversation, &message)?);
        assert!(
            store
                .claim_contract_workflow_attempt(&conversation, &message)?
                .is_none()
        );
        for table in ["contract_workflow_steps", "contract_workflow_scopes"] {
            let count: i64 = store.connection.query_row(
                &format!("SELECT COUNT(*) FROM {table}"),
                [],
                |row| row.get(0),
            )?;
            assert_eq!(count, 0);
        }
    }
    Ok(())
}
#[test]
fn parallel_pre_unlock_claims_have_one_durable_winner_across_connections()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("state.sqlite");
    let left = setup(&path)?;
    let right = ApplicationStore::open(&path)?;
    let barrier = std::sync::Barrier::new(2);
    let results = std::thread::scope(|scope| {
        let barrier = &barrier;
        let left = scope.spawn(move || {
            barrier.wait();
            let (conversation, message) = turn();
            left.claim_contract_workflow_attempt(&conversation, &message)
                .map(|attempt| attempt.is_some())
        });
        let right = scope.spawn(move || {
            barrier.wait();
            let (conversation, message) = turn();
            right
                .claim_contract_workflow_attempt(&conversation, &message)
                .map(|attempt| attempt.is_some())
        });
        (left.join(), right.join())
    });
    assert_ne!(
        results.0.map_err(|_| "left panicked")??,
        results.1.map_err(|_| "right panicked")??
    );
    let store = ApplicationStore::open(&path)?;
    assert!(
        store
            .claim_contract_workflow_attempt(&turn().0, &turn().1)?
            .is_none()
    );
    Ok(())
}
