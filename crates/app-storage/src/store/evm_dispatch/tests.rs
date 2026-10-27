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
#[test]
fn restart_at_each_dispatch_boundary_never_retries_or_traps_proven_unsent_work()
-> Result<(), Box<dyn std::error::Error>> {
    for stage in 0..4 {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("state.sqlite");
        let record = candidate(1);
        {
            let mut store = ApplicationStore::open(&path)?;
            if stage >= 1 {
                store.reserve_evm_transaction(&record, "attempt-one")?;
            }
            if stage >= 2 {
                assert!(store.mark_evm_dispatched(&record.transaction_hash, "attempt-one")?);
            }
            // Stage 3 represents an interrupted/unknown RPC send. Its durable
            // evidence must be identical to the may-send boundary, not guessed.
        }
        let mut store = ApplicationStore::open(&path)?;
        match stage {
            0 => assert!(store.evm_transactions()?.is_empty()),
            1 => {
                assert_eq!(
                    store.evm_dispatch_phase(&record.transaction_hash)?,
                    Some(StoredEvmDispatchPhase::Reserved)
                );
                assert_eq!(
                    store.retire_evm_reservations(&record.network, &record.sender)?,
                    1
                );
                assert!(!store.mark_evm_dispatched(&record.transaction_hash, "attempt-one")?);
                assert!(store.evm_transactions()?.is_empty());
                store.reserve_evm_transaction(&candidate(2), "attempt-one")?;
            }
            _ => {
                assert_eq!(
                    store.evm_dispatch_phase(&record.transaction_hash)?,
                    Some(StoredEvmDispatchPhase::Dispatched)
                );
                assert_eq!(
                    store.retire_evm_reservations(&record.network, &record.sender)?,
                    0
                );
                assert!(!store.mark_evm_dispatched(&record.transaction_hash, "attempt-one")?);
                assert_eq!(store.evm_transactions()?, vec![record.clone()]);
                assert!(store.acknowledge_evm_uncertainty(&record.transaction_hash)?);
                assert_eq!(
                    store.evm_dispatch_phase(&record.transaction_hash)?,
                    Some(StoredEvmDispatchPhase::Acknowledged)
                );
                assert_eq!(store.evm_transactions()?, vec![record.clone()]);
                assert!(
                    store
                        .reserve_evm_transaction(&record, "attempt-one")
                        .is_err()
                );
                assert!(!store.mark_evm_dispatched(&record.transaction_hash, "attempt-one")?);
                store.reserve_evm_transaction(&candidate(2), "attempt-one")?;
            }
        }
    }
    Ok(())
}
#[test]
fn reservation_and_marker_storage_faults_cannot_publish_send_authority()
-> Result<(), Box<dyn std::error::Error>> {
    let mut store = ApplicationStore::open_in_memory()?;
    let record = candidate(3);
    store.connection.execute_batch("CREATE TRIGGER fail_reservation BEFORE INSERT ON evm_transfer_dispatches BEGIN SELECT RAISE(ABORT,'fault'); END;")?;
    assert!(
        store
            .reserve_evm_transaction(&record, "attempt-one")
            .is_err()
    );
    assert!(store.evm_transactions()?.is_empty());
    store
        .connection
        .execute_batch("DROP TRIGGER fail_reservation;")?;
    store.reserve_evm_transaction(&record, "attempt-one")?;
    store.connection.execute_batch("CREATE TRIGGER fail_marker BEFORE UPDATE ON evm_transfer_dispatches BEGIN SELECT RAISE(ABORT,'fault'); END;")?;
    assert!(
        store
            .mark_evm_dispatched(&record.transaction_hash, "attempt-one")
            .is_err()
    );
    assert_eq!(
        store.evm_dispatch_phase(&record.transaction_hash)?,
        Some(StoredEvmDispatchPhase::Reserved)
    );
    assert_eq!(
        store.retire_evm_reservations(&record.network, &record.sender)?,
        1
    );
    Ok(())
}
#[test]
fn prior_schema_twenty_upgrades_without_erasing_or_inventing_dispatch_facts()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("old-profile.sqlite");
    let record = candidate(4);
    {
        let store = ApplicationStore::open(&path)?;
        store.save_evm_transaction(&record)?;
        store.connection.execute_batch("DROP TABLE evm_transfer_dispatches; DELETE FROM schema_migrations WHERE version=21; PRAGMA user_version=20;")?;
    }
    let store = ApplicationStore::open(&path)?;
    assert_eq!(store.schema_version()?, 21);
    assert_eq!(store.evm_transactions()?, vec![record.clone()]);
    assert_eq!(store.evm_dispatch_phase(&record.transaction_hash)?, None);
    assert_eq!(
        store.retire_evm_reservations(&record.network, &record.sender)?,
        0
    );
    assert!(store.acknowledge_evm_uncertainty(&record.transaction_hash)?);
    assert_eq!(store.evm_transactions()?, vec![record]);
    Ok(())
}
#[test]
fn cancelled_pre_send_cleanup_keeps_other_unknown_submissions()
-> Result<(), Box<dyn std::error::Error>> {
    let mut store = ApplicationStore::open_in_memory()?;
    let unsent = candidate(5);
    let unknown = candidate(6);
    store.reserve_evm_transaction(&unsent, "attempt-one")?;
    assert!(store.mark_evm_dispatched(&unsent.transaction_hash, "attempt-one")?);
    store.reserve_evm_transaction(&unknown, "attempt-one")?;
    assert!(store.mark_evm_dispatched(&unknown.transaction_hash, "attempt-one")?);
    store.discard_unsent_evm_transaction(&unsent.transaction_hash, "attempt-one")?;
    assert_eq!(store.evm_transactions()?, vec![unknown]);
    Ok(())
}

#[test]
fn a_retired_workers_identity_cannot_dispatch_or_delete_a_new_same_hash_reservation()
-> Result<(), Box<dyn std::error::Error>> {
    let mut store = ApplicationStore::open_in_memory()?;
    let record = candidate(7);
    store.reserve_evm_transaction(&record, "old-attempt")?;
    assert_eq!(
        store.retire_evm_reservations(&record.network, &record.sender)?,
        1
    );
    store.reserve_evm_transaction(&record, "new-attempt")?;
    assert!(!store.mark_evm_dispatched(&record.transaction_hash, "old-attempt")?);
    store.discard_unsent_evm_transaction(&record.transaction_hash, "old-attempt")?;
    assert_eq!(store.evm_transactions()?, vec![record.clone()]);
    assert!(store.mark_evm_dispatched(&record.transaction_hash, "new-attempt")?);
    Ok(())
}
