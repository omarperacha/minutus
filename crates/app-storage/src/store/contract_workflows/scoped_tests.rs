use super::*;
use crate::{ApplicationStore, StorageError, StoredEvmTransaction, StoredEvmTransactionState};
use ipc_types::{ConversationId, MessageId};
use serde_json::json;
fn setup() -> Result<ApplicationStore, StorageError> {
    let store = ApplicationStore::open_in_memory()?;
    store.connection.execute(
        "INSERT INTO projects(id,name,created_at,updated_at) VALUES ('p','P','now','now')",
        [],
    )?;
    store.connection.execute("INSERT INTO conversations(id,title,created_at,updated_at,project_id) VALUES ('c','C','now','now','p')",[])?;
    store.connection.execute("INSERT INTO messages(id,conversation_id,position,role,parts_json,created_at) VALUES ('m','c',0,'user','[]','now')",[])?;
    Ok(store)
}
const ACCOUNT: &str = "0x1111111111111111111111111111111111111111";
const TOKEN: &str = "0x2222222222222222222222222222222222222222";
const SPENDER: &str = "0x3333333333333333333333333333333333333333";
fn scope() -> StoredWorkflowScope {
    StoredWorkflowScope {
        version: 1,
        network: "eip155:8453".into(),
        account: ACCOUNT.into(),
        steps: vec![
            StoredWorkflowIntent {
                target: TOKEN.into(),
                profile_id: "fixture-ledger-v1".into(),
                profile_revision: 1,
                profile_commitment: "a".repeat(64),
                operation: "approve(address,uint256)".into(),
                arguments: vec![json!(SPENDER), json!("7")],
                value_wei: "0".into(),
            },
            StoredWorkflowIntent {
                target: SPENDER.into(),
                profile_id: "fixture-pull-v1".into(),
                profile_revision: 1,
                profile_commitment: "b".repeat(64),
                operation: "pull(address,uint256)".into(),
                arguments: vec![json!(TOKEN), json!("7")],
                value_wei: "0".into(),
            },
        ],
    }
}
fn observer() -> StoredContractObserver {
    StoredContractObserver {
        profile_id: "fixture-ledger-v1".into(),
        profile_revision: 1,
        profile_commitment: "a".repeat(64),
        predicate: binding::StoredObserverPredicate::Allowance {
            token: TOKEN.into(),
            owner: ACCOUNT.into(),
            spender: SPENDER.into(),
            amount: "7".into(),
        },
    }
}
fn candidate(intent: &str) -> StoredWorkflowCandidate {
    StoredWorkflowCandidate {
        intent_commitment: intent.into(),
        candidate_digest: format!("0x{}", "c".repeat(64)),
        signing_digest: format!("0x{}", "d".repeat(64)),
        source_commitment: "e".repeat(64),
        effects_commitment: "f".repeat(64),
        financial_commitment: "1".repeat(64),
        nonce: "3".into(),
        block_number: "42".into(),
        block_hash: format!("0x{}", "2".repeat(64)),
        observers: vec![observer()],
    }
}
fn record(c: &StoredWorkflowCandidate) -> StoredEvmTransaction {
    StoredEvmTransaction {
        transaction_hash: format!("0x{}", "3".repeat(64)),
        network: "eip155:8453".into(),
        sender: ACCOUNT.into(),
        nonce: c.nonce.clone(),
        candidate_digest: c.candidate_digest.clone(),
        state: StoredEvmTransactionState::Pending,
        block_number: None,
        block_hash: None,
        submitted_at: "now".into(),
        updated_at: "now".into(),
    }
}
fn observation(
    c: &StoredWorkflowCandidate,
    r: &StoredEvmTransaction,
) -> Result<StoredWorkflowObservation, StorageError> {
    Ok(StoredWorkflowObservation {
        candidate_commitment: c.commitment()?,
        transaction_hash: r.transaction_hash.clone(),
        receipt_block_number: "43".into(),
        receipt_block_hash: format!("0x{}", "4".repeat(64)),
        state_block_number: "43".into(),
        state_block_hash: format!("0x{}", "4".repeat(64)),
        receipt_commitment: "5".repeat(64),
        proof_commitment: "6".repeat(64),
        results: vec![StoredObserverResult {
            observer: observer(),
            value: "7".into(),
        }],
    })
}
#[test]
fn scoped_intent_is_immutable_while_candidate_context_is_fresh()
-> Result<(), Box<dyn std::error::Error>> {
    let mut store = setup()?;
    let scope = scope();
    let id = store
        .create_scoped_contract_workflow(
            &ConversationId("c".into()),
            &MessageId("m".into()),
            &scope,
        )?
        .ok_or("scope")?;
    assert_eq!(store.contract_workflow_scope(&id)?, Some(scope.clone()));
    let claim = store.claim_contract_step(&id)?.ok_or("claim")?;
    let c = candidate(claim.intent_commitment());
    assert!(store.bind_contract_step_candidate(&claim, &c)?);
    let mut refreshed = c.clone();
    refreshed.nonce = "4".into();
    refreshed.block_number = "44".into();
    assert!(!store.bind_contract_step_candidate(&claim, &refreshed)?);
    assert_eq!(store.contract_step_candidate(&id, 0)?, Some(c));
    assert!(
        store
            .connection
            .execute("UPDATE contract_workflow_scopes SET scope_json='{}'", [])
            .is_err()
    );
    Ok(())
}
#[test]
fn workflow_dispatch_is_atomic_with_nonce_and_verified_prerequisite()
-> Result<(), Box<dyn std::error::Error>> {
    let mut store = setup()?;
    let id = store
        .create_scoped_contract_workflow(
            &ConversationId("c".into()),
            &MessageId("m".into()),
            &scope(),
        )?
        .ok_or("scope")?;
    let step = store.claim_contract_step(&id)?.ok_or("step")?;
    let c = candidate(step.intent_commitment());
    assert!(store.bind_contract_step_candidate(&step, &c)?);
    assert!(
        store
            .record_contract_step(
                &step,
                ContractStepState::Reviewing,
                ContractStepState::Dispatching,
                None
            )
            .is_err()
    );
    let r = record(&c);
    let nonce = store
        .claim_evm_nonce(&r.network, &r.sender, &r.nonce, &r.candidate_digest, "one")?
        .ok_or("nonce")?;
    store.reserve_evm_transaction(&r, &nonce)?;
    assert!(store.mark_contract_workflow_dispatched(
        &step,
        &c.commitment()?,
        &r.transaction_hash,
        &nonce
    )?);
    assert!(!store.mark_contract_workflow_dispatched(
        &step,
        &c.commitment()?,
        &r.transaction_hash,
        &nonce
    )?);
    assert!(!store.release_unsent_evm_nonce(&nonce)?);
    assert!(store.record_contract_step(
        &step,
        ContractStepState::Dispatching,
        ContractStepState::Submitted,
        Some(&r.transaction_hash)
    )?);
    assert!(store.record_contract_step(
        &step,
        ContractStepState::Submitted,
        ContractStepState::ConfirmedUnverified,
        Some(&r.transaction_hash)
    )?);
    let evidence = observation(&c, &r)?;
    assert!(store.record_contract_step_observation(&step, &evidence)?);
    assert!(store.claim_contract_step(&id)?.is_none());
    assert!(
        store
            .verify_contract_step_postcondition(&step, &r.transaction_hash, &evidence.commitment()?)
            .is_err()
    );
    assert!(store.apply_verified_contract_step_observation(&step, &evidence)?);
    assert!(store.claim_contract_step(&id)?.is_some());
    Ok(())
}
#[test]
fn missing_or_unmet_observer_and_new_turn_cannot_release_successor()
-> Result<(), Box<dyn std::error::Error>> {
    let mut store = setup()?;
    let id = store
        .create_scoped_contract_workflow(
            &ConversationId("c".into()),
            &MessageId("m".into()),
            &scope(),
        )?
        .ok_or("scope")?;
    let step = store.claim_contract_step(&id)?.ok_or("step")?;
    let c = candidate(step.intent_commitment());
    store.bind_contract_step_candidate(&step, &c)?;
    let r = record(&c);
    let n = store
        .claim_evm_nonce(&r.network, &r.sender, &r.nonce, &r.candidate_digest, "one")?
        .ok_or("nonce")?;
    store.reserve_evm_transaction(&r, &n)?;
    assert!(store.contract_step_preparation_is_current(&step, &c.commitment()?)?);
    store.connection.execute("INSERT INTO messages(id,conversation_id,position,role,parts_json,created_at) VALUES ('new','c',1,'user','[]','now')",[])?;
    assert!(!store.contract_step_preparation_is_current(&step, &c.commitment()?)?);
    assert!(!store.mark_contract_workflow_dispatched(
        &step,
        &c.commitment()?,
        &r.transaction_hash,
        &n
    )?);
    assert!(store.evm_nonce_is_current(&n)?);
    assert_eq!(
        store.contract_workflow_steps(&id)?[0].state,
        ContractStepState::Reviewing
    );
    Ok(())
}

fn dispatched(
    store: &mut ApplicationStore,
) -> Result<
    (
        ContractWorkflowId,
        ContractStepClaim,
        StoredWorkflowCandidate,
        StoredEvmTransaction,
    ),
    Box<dyn std::error::Error>,
> {
    let id = store
        .create_scoped_contract_workflow(
            &ConversationId("c".into()),
            &MessageId("m".into()),
            &scope(),
        )?
        .ok_or("scope")?;
    let step = store.claim_contract_step(&id)?.ok_or("step")?;
    let c = candidate(step.intent_commitment());
    store.bind_contract_step_candidate(&step, &c)?;
    let r = record(&c);
    let n = store
        .claim_evm_nonce(
            &r.network,
            &r.sender,
            &r.nonce,
            &r.candidate_digest,
            "first",
        )?
        .ok_or("nonce")?;
    store.reserve_evm_transaction(&r, &n)?;
    assert!(store.mark_contract_workflow_dispatched(
        &step,
        &c.commitment()?,
        &r.transaction_hash,
        &n
    )?);
    Ok((id, step, c, r))
}
fn confirmed(
    store: &mut ApplicationStore,
    step: &ContractStepClaim,
    r: &StoredEvmTransaction,
) -> Result<(), StorageError> {
    assert!(store.record_contract_step(
        step,
        ContractStepState::Dispatching,
        ContractStepState::Submitted,
        Some(&r.transaction_hash)
    )?);
    assert!(store.record_contract_step(
        step,
        ContractStepState::Submitted,
        ContractStepState::ConfirmedUnverified,
        Some(&r.transaction_hash)
    )?);
    Ok(())
}
#[test]
fn workflow_storage_fault_rolls_back_the_nonce_send_fence() -> Result<(), Box<dyn std::error::Error>>
{
    let mut store = setup()?;
    let id = store
        .create_scoped_contract_workflow(
            &ConversationId("c".into()),
            &MessageId("m".into()),
            &scope(),
        )?
        .ok_or("scope")?;
    let step = store.claim_contract_step(&id)?.ok_or("step")?;
    let c = candidate(step.intent_commitment());
    store.bind_contract_step_candidate(&step, &c)?;
    let r = record(&c);
    let n = store
        .claim_evm_nonce(&r.network, &r.sender, &r.nonce, &r.candidate_digest, "one")?
        .ok_or("nonce")?;
    store.reserve_evm_transaction(&r, &n)?;
    store.connection.execute_batch("CREATE TRIGGER fail_workflow_dispatch BEFORE UPDATE OF state ON contract_workflow_steps WHEN NEW.state='dispatching' BEGIN SELECT RAISE(ABORT,'fault'); END;")?;
    assert!(
        store
            .mark_contract_workflow_dispatched(&step, &c.commitment()?, &r.transaction_hash, &n)
            .is_err()
    );
    assert!(store.evm_nonce_is_current(&n)?);
    assert_eq!(
        store.evm_dispatch_phase(&r.transaction_hash)?,
        Some(crate::StoredEvmDispatchPhase::Reserved)
    );
    assert_eq!(
        store.contract_workflow_steps(&id)?[0].state,
        ContractStepState::Reviewing
    );
    assert!(store.release_unsent_evm_nonce(&n)?);
    Ok(())
}
#[test]
fn raw_observation_or_receipt_success_cannot_credit_unmet_or_changed_predicates()
-> Result<(), Box<dyn std::error::Error>> {
    let mut store = setup()?;
    let (id, step, c, r) = dispatched(&mut store)?;
    confirmed(&mut store, &step, &r)?;
    let mut obs = observation(&c, &r)?;
    obs.results[0].value = "0".into();
    assert!(store.record_contract_step_observation(&step, &obs)?);
    assert!(
        store
            .apply_verified_contract_step_observation(&step, &obs)
            .is_err()
    );
    assert_eq!(
        store.contract_workflow_steps(&id)?[0].state,
        ContractStepState::ConfirmedUnverified
    );
    assert!(store.claim_contract_step(&id)?.is_none());
    let mut obs = observation(&c, &r)?;
    obs.results[0].observer.profile_commitment = "9".repeat(64);
    assert!(store.record_contract_step_observation(&step, &obs).is_err());
    Ok(())
}
#[test]
fn reorg_retires_a_prepared_successor_before_its_nonce_can_dispatch()
-> Result<(), Box<dyn std::error::Error>> {
    let mut store = setup()?;
    let (id, first, c, r) = dispatched(&mut store)?;
    confirmed(&mut store, &first, &r)?;
    store.apply_verified_contract_step_observation(&first, &observation(&c, &r)?)?;
    let second = store.claim_contract_step(&id)?.ok_or("second")?;
    let mut next = candidate(second.intent_commitment());
    next.nonce = "4".into();
    next.candidate_digest = format!("0x{}", "8".repeat(64));
    store.bind_contract_step_candidate(&second, &next)?;
    let mut record = record(&next);
    record.transaction_hash = format!("0x{}", "9".repeat(64));
    let n = store
        .claim_evm_nonce(
            &record.network,
            &record.sender,
            &record.nonce,
            &record.candidate_digest,
            "second",
        )?
        .ok_or("nonce")?;
    store.reserve_evm_transaction(&record, &n)?;
    assert!(store.record_contract_step(
        &first,
        ContractStepState::Verified,
        ContractStepState::Uncertain,
        None
    )?);
    assert!(!store.mark_contract_workflow_dispatched(
        &second,
        &next.commitment()?,
        &record.transaction_hash,
        &n
    )?);
    assert!(store.evm_nonce_is_current(&n)?);
    assert_eq!(
        store.contract_workflow_steps(&id)?[1].state,
        ContractStepState::Interrupted
    );
    assert!(store.claim_contract_step(&id)?.is_none());
    Ok(())
}
#[test]
fn missing_scoped_context_cannot_downgrade_to_legacy_or_credit_a_step()
-> Result<(), Box<dyn std::error::Error>> {
    let mut store = setup()?;
    let id = store
        .create_scoped_contract_workflow(
            &ConversationId("c".into()),
            &MessageId("m".into()),
            &scope(),
        )?
        .ok_or("scope")?;
    store
        .connection
        .execute("DELETE FROM contract_workflow_scopes", [])?;
    assert!(store.contract_workflow_scope(&id).is_err());
    assert!(store.claim_contract_step(&id).is_err());
    Ok(())
}
#[test]
fn restart_preserves_scoped_observer_context_but_retires_all_execution_authority()
-> Result<(), Box<dyn std::error::Error>> {
    let mut store = setup()?;
    let (id, first, c, r) = dispatched(&mut store)?;
    confirmed(&mut store, &first, &r)?;
    store.apply_verified_contract_step_observation(&first, &observation(&c, &r)?)?;
    let second = store.claim_contract_step(&id)?.ok_or("second")?;
    let c2 = candidate(second.intent_commitment());
    store.bind_contract_step_candidate(&second, &c2)?;
    store.interrupt_contract_workflows_after_restart()?;
    assert!(store.claim_contract_step(&id)?.is_none());
    assert_eq!(store.contract_step_observations(&id, 0)?.len(), 1);
    assert_eq!(store.contract_step_candidate(&id, 1)?, Some(c2));
    assert_eq!(
        store.contract_workflow_steps(&id)?[1].state,
        ContractStepState::Interrupted
    );
    assert_eq!(store.evm_transactions()?.len(), 1);
    Ok(())
}

#[test]
fn callback_false_cleanup_retires_both_owners_but_never_the_retained_hash()
-> Result<(), Box<dyn std::error::Error>> {
    let mut store = setup()?;
    let id = store
        .create_scoped_contract_workflow(
            &ConversationId("c".into()),
            &MessageId("m".into()),
            &scope(),
        )?
        .ok_or("scope")?;
    let step = store.claim_contract_step(&id)?.ok_or("step")?;
    let c = candidate(step.intent_commitment());
    store.bind_contract_step_candidate(&step, &c)?;
    let r = record(&c);
    let n = store
        .claim_evm_nonce(
            &r.network,
            &r.sender,
            &r.nonce,
            &r.candidate_digest,
            "first",
        )?
        .ok_or("nonce")?;
    store.reserve_evm_transaction(&r, &n)?;
    assert!(store.mark_contract_workflow_dispatched(
        &step,
        &c.commitment()?,
        &r.transaction_hash,
        &n
    )?);
    assert!(store.discard_known_unsent_contract_workflow(&step, &r.transaction_hash, &n)?);
    let steps = store.contract_workflow_steps(&id)?;
    assert_eq!(steps[0].state, ContractStepState::Cancelled);
    assert!(steps[0].transaction_hash.is_none());
    assert!(store.claim_contract_step(&id)?.is_none());
    assert!(store.evm_transactions()?.is_empty());
    assert!(!store.mark_evm_dispatched(&r.transaction_hash, &n)?);
    assert_eq!(
        store
            .connection
            .query_row("SELECT count(*) FROM evm_nonce_hashes", [], |row| row
                .get::<_, u32>(0))?,
        1
    );
    Ok(())
}

#[test]
fn fresh_verified_context_cannot_resurrect_a_reorganised_step()
-> Result<(), Box<dyn std::error::Error>> {
    let mut store = setup()?;
    let (id, first, c, r) = dispatched(&mut store)?;
    confirmed(&mut store, &first, &r)?;
    let original = observation(&c, &r)?;
    assert!(store.apply_verified_contract_step_observation(&first, &original)?);
    let mut fresh = original.clone();
    fresh.state_block_number = "115".into();
    fresh.state_block_hash = format!("0x{}", "5".repeat(64));
    fresh.proof_commitment = "6".repeat(64);
    store.refresh_verified_contract_step(&id, 0, first.intent_commitment(), &fresh)?;
    // Identical already-observed evidence is a normal idempotent refresh, not
    // a failed state CAS or permission to revive an invalidated prerequisite.
    store.refresh_verified_contract_step(&id, 0, first.intent_commitment(), &fresh)?;
    assert_eq!(store.contract_step_observations(&id, 0)?.len(), 2);
    assert!(store.record_contract_step(
        &first,
        ContractStepState::Verified,
        ContractStepState::Uncertain,
        None
    )?);
    let mut later = fresh;
    later.state_block_number = "116".into();
    later.proof_commitment = "7".repeat(64);
    assert!(
        store
            .refresh_verified_contract_step(&id, 0, first.intent_commitment(), &later)
            .is_err()
    );
    assert_eq!(store.contract_step_observations(&id, 0)?.len(), 2);
    assert_eq!(
        store.contract_workflow_steps(&id)?[0].state,
        ContractStepState::Uncertain
    );
    assert!(store.claim_contract_step(&id)?.is_none());
    Ok(())
}
