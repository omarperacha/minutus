use super::*;
use crate::{ApplicationStore, StorageError};
use ContractStepState as S;
use ipc_types::{ConversationId, MessageId};

fn setup(path: &std::path::Path) -> Result<ApplicationStore, StorageError> {
    let store = ApplicationStore::open(path)?;
    store.connection.execute("INSERT INTO projects (id,name,created_at,updated_at) VALUES ('project','Project','now','now')",[])?;
    store.connection.execute("INSERT INTO conversations (id,title,created_at,updated_at,project_id) VALUES ('chat','Chat','now','now','project')",[])?;
    store.connection.execute("INSERT INTO messages (id,conversation_id,position,role,parts_json,created_at) VALUES ('turn','chat',0,'user','[]','now')",[])?;
    Ok(store)
}
fn create(store: &mut ApplicationStore) -> Result<ContractWorkflowId, Box<dyn std::error::Error>> {
    store
        .create_contract_workflow(
            &ConversationId("chat".into()),
            &MessageId("turn".into()),
            &"a".repeat(64),
            &["b".repeat(64), "c".repeat(64)],
        )?
        .ok_or_else(|| "workflow missing".into())
}
fn hash() -> String {
    format!("0x{}", "d".repeat(64))
}

#[test]
fn sequential_steps_require_verified_prerequisites_and_stop_after_denial()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let mut store = setup(&dir.path().join("state.sqlite"))?;
    let id = create(&mut store)?;
    assert!(!store.claim_chat_transfer(&ConversationId("chat".into()), &MessageId("turn".into()))?);
    assert!(
        store
            .create_contract_workflow(
                &ConversationId("chat".into()),
                &MessageId("turn".into()),
                &"e".repeat(64),
                &["f".repeat(64)]
            )?
            .is_none()
    );
    let first = store.claim_contract_step(&id)?.ok_or("first missing")?;
    assert!(store.claim_contract_step(&id)?.is_none());
    assert!(store.record_contract_step(&first, S::Reviewing, S::Dispatching, None)?);
    assert!(!store.record_contract_step(&first, S::Reviewing, S::Dispatching, None)?);
    assert!(store.record_contract_step(&first, S::Dispatching, S::Submitted, Some(&hash()))?);
    assert!(store.record_contract_step(
        &first,
        S::Submitted,
        S::ConfirmedUnverified,
        Some(&hash())
    )?);
    assert!(
        store.claim_contract_step(&id)?.is_none(),
        "receipt success is not a postcondition"
    );
    assert!(!store.verify_contract_step_postcondition(
        &first,
        &format!("0x{}", "e".repeat(64)),
        &"f".repeat(64)
    )?);
    assert!(store.verify_contract_step_postcondition(&first, &hash(), &"f".repeat(64))?);
    let second = store.claim_contract_step(&id)?.ok_or("second missing")?;
    assert!(store.record_contract_step(&second, S::Reviewing, S::Denied, None)?);
    assert!(store.claim_contract_step(&id)?.is_none());
    let steps = store.contract_workflow_steps(&id)?;
    assert_eq!(
        steps.iter().map(|step| step.state).collect::<Vec<_>>(),
        vec![S::Verified, S::Denied]
    );
    assert_eq!(steps[0].transaction_hash, Some(hash()));
    // Previous allowance/other effects are retained; no automatic cleanup exists.
    assert!(
        store
            .record_contract_step(&second, S::Denied, S::Reviewing, None)
            .is_err()
    );
    Ok(())
}

#[test]
fn restart_retires_all_unsent_steps_and_preserves_uncertain_dispatch()
-> Result<(), Box<dyn std::error::Error>> {
    for dispatched in [false, true] {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("state.sqlite");
        let mut store = setup(&path)?;
        let id = create(&mut store)?;
        let first = store.claim_contract_step(&id)?.ok_or("first missing")?;
        if dispatched {
            assert!(store.record_contract_step(
                &first,
                S::Reviewing,
                S::Dispatching,
                Some(&hash())
            )?);
        }
        drop(store);
        let mut store = ApplicationStore::open(&path)?;
        store.interrupt_contract_workflows_after_restart()?;
        let restored = store
            .contract_workflow_for_turn(&ConversationId("chat".into()), &MessageId("turn".into()))?
            .ok_or("lookup missing")?;
        assert_eq!(restored, id);
        assert!(store.claim_contract_step(&id)?.is_none());
        let steps = store.contract_workflow_steps(&id)?;
        assert_eq!(
            steps[0].state,
            if dispatched {
                S::Uncertain
            } else {
                S::Interrupted
            }
        );
        assert_eq!(steps[1].state, S::Interrupted);
        assert!(
            store
                .create_contract_workflow(
                    &ConversationId("chat".into()),
                    &MessageId("turn".into()),
                    &"e".repeat(64),
                    &["f".repeat(64)]
                )?
                .is_none()
        );
    }
    Ok(())
}

#[test]
fn concurrent_claims_and_aliases_cannot_consume_same_step_twice()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("state.sqlite");
    let mut left = setup(&path)?;
    let id = create(&mut left)?;
    let mut right = ApplicationStore::open(&path)?;
    let barrier = std::sync::Barrier::new(2);
    let (left, right) = std::thread::scope(|scope| {
        let barrier = &barrier;
        let id = &id;
        let a = scope.spawn(move || {
            barrier.wait();
            left.claim_contract_step(id)
        });
        let b = scope.spawn(move || {
            barrier.wait();
            right.claim_contract_step(id)
        });
        (a.join(), b.join())
    });
    assert_ne!(
        left.map_err(|_| "claim panicked")??.is_some(),
        right.map_err(|_| "claim panicked")??.is_some()
    );
    Ok(())
}

#[test]
fn invalid_scope_old_turn_and_standalone_claim_cannot_start_workflow()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let mut store = setup(&dir.path().join("state.sqlite"))?;
    assert!(
        store
            .create_contract_workflow(
                &ConversationId("chat".into()),
                &MessageId("turn".into()),
                "not a digest",
                &["b".repeat(64)]
            )
            .is_err()
    );
    assert!(store.claim_chat_transfer(&ConversationId("chat".into()), &MessageId("turn".into()))?);
    assert!(
        store
            .create_contract_workflow(
                &ConversationId("chat".into()),
                &MessageId("turn".into()),
                &"a".repeat(64),
                &["b".repeat(64)]
            )?
            .is_none()
    );
    Ok(())
}

#[test]
fn corrupt_missing_or_unproven_prerequisites_never_release_a_successor()
-> Result<(), Box<dyn std::error::Error>> {
    for corruption in [
        "UPDATE contract_workflow_steps SET state='verified' WHERE step_index=0",
        "DELETE FROM contract_workflow_steps WHERE step_index=0",
        "UPDATE contract_workflow_steps SET state='submitted' WHERE step_index=0",
        "UPDATE contract_workflow_steps SET commitment=printf('%064d',0) WHERE step_index=0",
        "UPDATE contract_workflows SET step_commitments_json='[]'",
    ] {
        let dir = tempfile::tempdir()?;
        let mut store = setup(&dir.path().join("state.sqlite"))?;
        let id = create(&mut store)?;
        store.connection.execute(corruption, [])?;
        assert!(store.claim_contract_step(&id).is_err(), "{corruption}");
        assert!(store.contract_workflow_steps(&id).is_err(), "{corruption}");
    }
    Ok(())
}

#[test]
fn deleted_workflow_identity_cannot_be_reused_by_a_stale_claim()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("state.sqlite");
    let mut store = setup(&path)?;
    let old_id = create(&mut store)?;
    let old_claim = store
        .claim_contract_step(&old_id)?
        .ok_or("missing old claim")?;
    store
        .connection
        .execute("DELETE FROM conversations WHERE id='chat'", [])?;
    store.connection.execute("INSERT INTO conversations (id,title,created_at,updated_at,project_id) VALUES ('chat','Chat','now','now','project')",[])?;
    store.connection.execute("INSERT INTO messages (id,conversation_id,position,role,parts_json,created_at) VALUES ('turn','chat',0,'user','[]','now')",[])?;
    let new_id = create(&mut store)?;
    assert_ne!(new_id, old_id);
    let _new_claim = store
        .claim_contract_step(&new_id)?
        .ok_or("missing new claim")?;
    assert!(
        store
            .record_contract_step(&old_claim, S::Reviewing, S::Dispatching, None)
            .is_err()
    );
    assert_eq!(
        store.contract_workflow_steps(&new_id)?[0].state,
        S::Reviewing
    );
    Ok(())
}

#[test]
fn reorg_revokes_verified_evidence_and_never_automatically_retries()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let mut store = setup(&dir.path().join("state.sqlite"))?;
    let id = create(&mut store)?;
    let first = store.claim_contract_step(&id)?.ok_or("first missing")?;
    assert!(store.record_contract_step(&first, S::Reviewing, S::Dispatching, Some(&hash()))?);
    assert!(store.record_contract_step(&first, S::Dispatching, S::Submitted, Some(&hash()))?);
    assert!(store.record_contract_step(
        &first,
        S::Submitted,
        S::ConfirmedUnverified,
        Some(&hash())
    )?);
    assert!(store.verify_contract_step_postcondition(&first, &hash(), &"e".repeat(64))?);
    assert!(store.record_contract_step(&first, S::Verified, S::Uncertain, None)?);
    assert!(store.claim_contract_step(&id)?.is_none());
    let steps = store.contract_workflow_steps(&id)?;
    assert!(steps[0].evidence_commitment.is_none());
    assert_eq!(steps[0].transaction_hash, Some(hash()));
    assert_eq!(steps[1].state, S::Interrupted);
    assert!(
        store
            .record_contract_step(&first, S::Uncertain, S::Dispatching, Some(&hash()))
            .is_err()
    );
    Ok(())
}

#[test]
fn prerequisite_reorg_during_successor_review_retires_the_pending_step()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let mut store = setup(&dir.path().join("state.sqlite"))?;
    let id = create(&mut store)?;
    let first = store.claim_contract_step(&id)?.ok_or("first missing")?;
    assert!(store.record_contract_step(&first, S::Reviewing, S::Dispatching, Some(&hash()))?);
    assert!(store.record_contract_step(&first, S::Dispatching, S::Submitted, Some(&hash()))?);
    assert!(store.record_contract_step(
        &first,
        S::Submitted,
        S::ConfirmedUnverified,
        Some(&hash())
    )?);
    assert!(store.verify_contract_step_postcondition(&first, &hash(), &"e".repeat(64))?);
    let second = store.claim_contract_step(&id)?.ok_or("second missing")?;
    assert!(store.record_contract_step(&first, S::Verified, S::Uncertain, None)?);
    assert!(!store.record_contract_step(&second, S::Reviewing, S::Dispatching, None)?);
    assert_eq!(store.contract_workflow_steps(&id)?[1].state, S::Interrupted);
    // Reconciling the earlier transaction cannot revive the retired review.
    assert!(store.record_contract_step(
        &first,
        S::Uncertain,
        S::ConfirmedUnverified,
        Some(&hash())
    )?);
    assert!(store.verify_contract_step_postcondition(&first, &hash(), &"e".repeat(64))?);
    assert!(!store.record_contract_step(&second, S::Reviewing, S::Dispatching, None)?);
    assert!(store.claim_contract_step(&id)?.is_none());
    Ok(())
}

#[test]
fn schema_twenty_one_upgrade_preserves_standalone_turn_tombstones()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("old-profile.sqlite");
    {
        let store = setup(&path)?;
        assert!(
            store.claim_chat_transfer(&ConversationId("chat".into()), &MessageId("turn".into()))?
        );
        store.connection.execute_batch("DROP TRIGGER evm_transaction_nonce_conflict; DROP TRIGGER evm_transaction_identity_immutable; DROP TRIGGER evm_transaction_nonce_retention; DROP TRIGGER evm_dispatch_requires_nonce; DROP TRIGGER evm_dispatch_identity_immutable; DROP TRIGGER evm_dispatch_requires_fence; DROP TABLE evm_nonce_hashes; DROP TABLE evm_nonce_claims; DROP TRIGGER contract_scope_format_immutable; DROP TABLE contract_workflow_observations; DROP TABLE contract_workflow_candidates; DROP TABLE contract_workflow_scopes; DROP TABLE contract_workflow_steps; DROP TABLE contract_workflows; DELETE FROM schema_migrations WHERE version>=22; PRAGMA user_version=21;")?;
    }
    let mut store = ApplicationStore::open(&path)?;
    assert_eq!(store.schema_version()?, crate::SCHEMA_VERSION);
    assert!(
        store
            .contract_workflow_for_turn(&ConversationId("chat".into()), &MessageId("turn".into()))?
            .is_none()
    );
    assert!(!store.claim_chat_transfer(&ConversationId("chat".into()), &MessageId("turn".into()))?);
    assert!(
        store
            .create_contract_workflow(
                &ConversationId("chat".into()),
                &MessageId("turn".into()),
                &"a".repeat(64),
                &["b".repeat(64)]
            )?
            .is_none()
    );
    Ok(())
}
