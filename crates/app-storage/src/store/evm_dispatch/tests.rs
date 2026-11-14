use super::*;
fn candidate(byte: u8) -> StoredEvmTransaction {
    StoredEvmTransaction {
        transaction_hash: format!("0x{}", format!("{byte:02x}").repeat(32)),
        network: "eip155:8453".into(),
        sender: "0x1111111111111111111111111111111111111111".into(),
        nonce: "7".into(),
        candidate_digest: format!("0x{}", "22".repeat(32)),
        state: StoredEvmTransactionState::Pending,
        block_number: None,
        block_hash: None,
        submitted_at: "2026-10-27T00:00:00Z".into(),
        updated_at: "2026-10-27T00:00:00Z".into(),
    }
}
fn claim(
    store: &mut ApplicationStore,
    record: &StoredEvmTransaction,
    owner: &str,
) -> Result<EvmNonceClaim, Box<dyn std::error::Error>> {
    store
        .claim_evm_nonce(
            &record.network,
            &record.sender,
            &record.nonce,
            &record.candidate_digest,
            owner,
        )?
        .ok_or_else(|| "nonce busy".into())
}
#[test]
fn restart_at_each_dispatch_boundary_never_retries_or_traps_proven_unsent_work()
-> Result<(), Box<dyn std::error::Error>> {
    for stage in 0..4 {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("state.sqlite");
        let record = candidate(1);
        let mut owner = None;
        {
            let mut store = ApplicationStore::open(&path)?;
            if stage >= 1 {
                let nonce = claim(&mut store, &record, "attempt-one")?;
                store.reserve_evm_transaction(&record, &nonce)?;
                if stage >= 2 {
                    assert!(store.mark_evm_dispatched(&record.transaction_hash, &nonce)?);
                }
                owner = Some(nonce);
            }
        }
        let mut store = ApplicationStore::open(&path)?;
        if stage == 0 {
            assert!(store.evm_transactions()?.is_empty());
            continue;
        }
        let owner = owner.ok_or("claim")?;
        assert_eq!(
            store.retire_evm_reservations(&record.network, &record.sender)?,
            usize::from(stage == 1)
        );
        assert!(!store.mark_evm_dispatched(&record.transaction_hash, &owner)?);
        if stage == 1 {
            assert!(store.evm_transactions()?.is_empty());
            let next = candidate(2);
            let nonce = claim(&mut store, &next, "attempt-two")?;
            store.reserve_evm_transaction(&next, &nonce)?;
        } else {
            assert_eq!(
                store.evm_dispatch_phase(&record.transaction_hash)?,
                Some(StoredEvmDispatchPhase::Dispatched)
            );
            assert!(store.acknowledge_evm_uncertainty(&record.transaction_hash)?);
            assert_eq!(store.evm_transactions()?, vec![record.clone()]);
            assert!(
                store
                    .claim_evm_nonce(
                        &record.network,
                        &record.sender,
                        &record.nonce,
                        &record.candidate_digest,
                        "attempt-two"
                    )?
                    .is_none()
            );
            let mut next = candidate(2);
            next.nonce = "8".into();
            let nonce = claim(&mut store, &next, "attempt-two")?;
            store.reserve_evm_transaction(&next, &nonce)?;
        }
    }
    Ok(())
}
#[test]
fn reservation_and_marker_storage_faults_cannot_publish_send_authority()
-> Result<(), Box<dyn std::error::Error>> {
    let mut store = ApplicationStore::open_in_memory()?;
    let record = candidate(3);
    let nonce = claim(&mut store, &record, "attempt-one")?;
    store.connection.execute_batch("CREATE TRIGGER fail_reservation BEFORE INSERT ON evm_transfer_dispatches BEGIN SELECT RAISE(ABORT,'fault'); END;")?;
    assert!(store.reserve_evm_transaction(&record, &nonce).is_err());
    assert!(store.evm_transactions()?.is_empty());
    store
        .connection
        .execute_batch("DROP TRIGGER fail_reservation;")?;
    store.reserve_evm_transaction(&record, &nonce)?;
    store.connection.execute_batch("CREATE TRIGGER fail_marker BEFORE UPDATE ON evm_transfer_dispatches BEGIN SELECT RAISE(ABORT,'fault'); END;")?;
    assert!(
        store
            .mark_evm_dispatched(&record.transaction_hash, &nonce)
            .is_err()
    );
    assert_eq!(
        store.evm_dispatch_phase(&record.transaction_hash)?,
        Some(StoredEvmDispatchPhase::Reserved)
    );
    assert!(store.release_unsent_evm_nonce(&nonce)?);
    assert!(store.evm_transactions()?.is_empty());
    Ok(())
}
#[test]
fn cancelled_pre_send_cleanup_keeps_other_unknown_submissions()
-> Result<(), Box<dyn std::error::Error>> {
    let mut store = ApplicationStore::open_in_memory()?;
    let unsent = candidate(5);
    let mut unknown = candidate(6);
    unknown.nonce = "8".into();
    let first = claim(&mut store, &unsent, "attempt-one")?;
    store.reserve_evm_transaction(&unsent, &first)?;
    assert!(store.mark_evm_dispatched(&unsent.transaction_hash, &first)?);
    let second = claim(&mut store, &unknown, "attempt-two")?;
    store.reserve_evm_transaction(&unknown, &second)?;
    assert!(store.mark_evm_dispatched(&unknown.transaction_hash, &second)?);
    assert!(!store.release_unsent_evm_nonce(&first)?);
    store.discard_unsent_evm_transaction(&unsent.transaction_hash, &first)?;
    assert_eq!(store.evm_transactions()?, vec![unknown]);
    assert!(!store.release_unsent_evm_nonce(&second)?);
    assert!(claim(&mut store, &unsent, "attempt-three").is_ok());
    Ok(())
}
#[test]
fn a_retired_workers_identity_cannot_dispatch_or_delete_a_new_same_hash_reservation()
-> Result<(), Box<dyn std::error::Error>> {
    let mut store = ApplicationStore::open_in_memory()?;
    let record = candidate(7);
    let old = claim(&mut store, &record, "old-attempt")?;
    store.reserve_evm_transaction(&record, &old)?;
    assert_eq!(
        store.retire_evm_reservations(&record.network, &record.sender)?,
        1
    );
    let new = claim(&mut store, &record, "new-attempt")?;
    store.reserve_evm_transaction(&record, &new)?;
    assert!(!store.mark_evm_dispatched(&record.transaction_hash, &old)?);
    store.discard_unsent_evm_transaction(&record.transaction_hash, &old)?;
    assert_eq!(store.evm_transactions()?, vec![record.clone()]);
    assert!(store.mark_evm_dispatched(&record.transaction_hash, &new)?);
    Ok(())
}
