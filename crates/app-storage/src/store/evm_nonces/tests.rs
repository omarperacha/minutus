use super::*;
use crate::{StoredEvmTransaction, StoredEvmTransactionState};
fn record(byte: u8) -> StoredEvmTransaction {
    StoredEvmTransaction {
        transaction_hash: format!("0x{}", format!("{byte:02x}").repeat(32)),
        network: "eip155:8453".into(),
        sender: "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd".into(),
        nonce: "7".into(),
        candidate_digest: format!("0x{}", "22".repeat(32)),
        state: StoredEvmTransactionState::Pending,
        block_number: None,
        block_hash: None,
        submitted_at: "2026-10-30T00:00:00Z".into(),
        updated_at: "2026-10-30T00:00:00Z".into(),
    }
}
fn claim(
    store: &mut ApplicationStore,
    r: &StoredEvmTransaction,
    owner: &str,
) -> Result<EvmNonceClaim, Box<dyn std::error::Error>> {
    store
        .claim_evm_nonce(&r.network, &r.sender, &r.nonce, &r.candidate_digest, owner)?
        .ok_or_else(|| "nonce busy".into())
}
#[test]
fn competing_connections_and_account_spellings_cannot_duplicate_nonce()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("db.sqlite");
    let mut a = ApplicationStore::open(&path)?;
    let mut b = ApplicationStore::open(&path)?;
    let r = record(1);
    let first = claim(&mut a, &r, "wallet-review")?;
    let mut other = r.clone();
    other.sender = format!("0x{}", r.sender[2..].to_ascii_uppercase());
    other.candidate_digest = format!("0x{}", "33".repeat(32));
    assert!(claim(&mut b, &other, "chat-review").is_err());
    assert!(a.release_unsent_evm_nonce(&first)?);
    let next = claim(&mut b, &other, "chat-review")?;
    assert!(next.id > first.id);
    assert!(!a.release_unsent_evm_nonce(&first)?);
    assert!(
        a.claim_evm_nonce(
            &r.network,
            &r.sender,
            "8",
            &r.candidate_digest,
            "wallet-review"
        )?
        .is_none()
    );
    Ok(())
}
#[test]
fn a_managed_owner_binds_one_hash_and_rejects_conflicting_monitoring()
-> Result<(), Box<dyn std::error::Error>> {
    let mut s = ApplicationStore::open_in_memory()?;
    let r = record(1);
    let c = claim(&mut s, &r, "one")?;
    assert!(s.save_evm_transaction(&r).is_err());
    s.reserve_evm_transaction(&r, &c)?;
    let mut other = record(2);
    assert!(s.reserve_evm_transaction(&other, &c).is_err());
    assert!(s.save_evm_transaction(&other).is_err());
    other.transaction_hash = r.transaction_hash.clone();
    other.nonce = "8".into();
    assert!(s.save_evm_transaction(&other).is_err());
    assert!(s.mark_evm_dispatched(&r.transaction_hash, &c)?);
    assert!(!s.mark_evm_dispatched(&r.transaction_hash, &c)?);
    assert!(!s.release_unsent_evm_nonce(&c)?);
    Ok(())
}
#[test]
fn acknowledgement_history_deletion_and_failed_receipt_never_free_possible_send()
-> Result<(), Box<dyn std::error::Error>> {
    let mut s = ApplicationStore::open_in_memory()?;
    let mut r = record(1);
    let c = claim(&mut s, &r, "one")?;
    s.reserve_evm_transaction(&r, &c)?;
    assert!(s.mark_evm_dispatched(&r.transaction_hash, &c)?);
    assert!(s.acknowledge_evm_uncertainty(&r.transaction_hash)?);
    assert!(claim(&mut s, &r, "two").is_err());
    r.state = StoredEvmTransactionState::Failed;
    r.block_number = Some("42".into());
    r.block_hash = Some(format!("0x{}", "44".repeat(32)));
    s.save_evm_transaction(&r)?;
    s.connection.execute("DELETE FROM evm_transactions", [])?;
    assert!(claim(&mut s, &r, "two").is_err());
    assert!(
        s.connection
            .execute("DELETE FROM evm_nonce_claims", [])
            .is_err()
    );
    assert_eq!(
        s.connection
            .query_row("SELECT count(*) FROM evm_nonce_hashes", [], |row| row
                .get::<_, u32>(0))?,
        1
    );
    Ok(())
}
#[test]
fn invalid_nonce_bindings_and_storage_failure_never_create_ownership()
-> Result<(), Box<dyn std::error::Error>> {
    let mut s = ApplicationStore::open_in_memory()?;
    let r = record(1);
    for nonce in ["", "01", "-1", "1e2", "0x1"] {
        assert!(
            s.claim_evm_nonce(&r.network, &r.sender, nonce, &r.candidate_digest, "one")
                .is_err()
        );
    }
    s.connection.execute_batch("CREATE TRIGGER failed_claim BEFORE INSERT ON evm_nonce_claims BEGIN SELECT RAISE(ABORT,'fault'); END;")?;
    assert!(claim(&mut s, &r, "one").is_err());
    assert_eq!(
        s.connection
            .query_row("SELECT count(*) FROM evm_nonce_claims", [], |row| row
                .get::<_, u32>(0))?,
        0
    );
    Ok(())
}
