use super::*;

#[test]
fn filesystem_audit_metadata_is_bounded() -> Result<(), Box<dyn Error>> {
    let store = ApplicationStore::open_in_memory()?;
    store.append_audit_event(&AuditEvent {
        id: "event-2".to_owned(),
        kind: AuditEventKind::FilesystemTool,
        outcome: AuditOutcome::Succeeded,
        subject_id: None,
        operation_id: Some("operation-1".to_owned()),
        tool: Some(crate::AuditFilesystemTool::ReadText),
        workspace_grant_id: Some(WorkspaceGrantId("grant-1".to_owned())),
        relative_path: Some("src/lib.rs".to_owned()),
        permission_decision: crate::AuditPermissionDecision::Allowed,
        result_count: Some(1),
        truncated: Some(false),
        occurred_at: NOW.to_owned(),
    })?;
    let metadata = store.connection.query_row(
        "SELECT tool_name, workspace_grant_id, relative_path,
                    permission_decision, result_count, truncated
             FROM audit_events WHERE id = 'event-2'",
        [],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, u32>(4)?,
                row.get::<_, bool>(5)?,
            ))
        },
    )?;
    assert_eq!(
        metadata,
        (
            "filesystem.read_text".to_owned(),
            "grant-1".to_owned(),
            "src/lib.rs".to_owned(),
            "allowed".to_owned(),
            1,
            false,
        )
    );

    let mut invalid = AuditEvent {
        id: "event-3".to_owned(),
        kind: AuditEventKind::FilesystemTool,
        outcome: AuditOutcome::Failed,
        subject_id: None,
        operation_id: Some("operation-2".to_owned()),
        tool: Some(crate::AuditFilesystemTool::ReadText),
        workspace_grant_id: Some(WorkspaceGrantId("grant-1".to_owned())),
        relative_path: Some("/private/secret.txt".to_owned()),
        permission_decision: crate::AuditPermissionDecision::Allowed,
        result_count: None,
        truncated: None,
        occurred_at: NOW.to_owned(),
    };
    assert!(store.append_audit_event(&invalid).is_err());
    invalid.id = "event-4".to_owned();
    invalid.relative_path = Some("\\\\server\\secret.txt".to_owned());
    assert!(store.append_audit_event(&invalid).is_err());
    assert_eq!(store.audit_event_count()?, 1);
    Ok(())
}

#[test]
fn evm_monitoring_persists_only_public_restart_safe_metadata() -> Result<(), Box<dyn Error>> {
    let store = ApplicationStore::open_in_memory()?;
    let mut transaction = StoredEvmTransaction {
        transaction_hash: format!("0x{}", "11".repeat(32)),
        network: "eip155:84532".to_owned(),
        sender: "0x9858EfFD232B4033E47d90003D41EC34EcaEda94".to_owned(),
        nonce: "7".to_owned(),
        candidate_digest: format!("0x{}", "22".repeat(32)),
        state: StoredEvmTransactionState::Pending,
        block_number: None,
        block_hash: None,
        submitted_at: NOW.to_owned(),
        updated_at: NOW.to_owned(),
    };
    store.save_evm_transaction(&transaction)?;
    assert_eq!(store.evm_transactions()?, vec![transaction.clone()]);

    transaction.state = StoredEvmTransactionState::Confirmed;
    transaction.block_number = Some("42".to_owned());
    transaction.block_hash = Some(format!("0x{}", "33".repeat(32)));
    transaction.updated_at = "2026-08-14T12:01:00Z".to_owned();
    store.save_evm_transaction(&transaction)?;
    assert_eq!(store.evm_transactions()?, vec![transaction]);
    Ok(())
}

fn approval_finalization() -> StoredApprovalFinalization {
    StoredApprovalFinalization {
        approval_id: "11".repeat(32),
        commitment_digest: "22".repeat(32),
        signing_digest: "33".repeat(32),
        chain_id: "8453".to_owned(),
        account: "0x9858effd232b4033e47d90003d41ec34ecaeda94".to_owned(),
        session_id: "session-01".to_owned(),
        expires_at_unix: "1800000120".to_owned(),
        transaction_type: "eip1559:2".to_owned(),
        policy_revision: "2".to_owned(),
        disposition: "consumed".to_owned(),
        invalidation_reason: None,
        finalized_at_unix: "1800000001".to_owned(),
    }
}

#[test]
fn approval_finalization_is_atomic_across_database_connections() -> Result<(), Box<dyn Error>> {
    let database = NamedTempFile::new()?;
    drop(ApplicationStore::open(database.path())?);
    let path = database.path().to_owned();
    let barrier = Arc::new(Barrier::new(8));
    let handles = (0..8)
        .map(|_| {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(
                move || -> Result<StoredApprovalFinalizationOutcome, String> {
                    let store = ApplicationStore::open(path).map_err(|error| error.to_string())?;
                    barrier.wait();
                    store
                        .finalize_transaction_approval(&approval_finalization())
                        .map_err(|error| error.to_string())
                },
            )
        })
        .collect::<Vec<_>>();
    let outcomes = handles
        .into_iter()
        .map(|handle| {
            handle
                .join()
                .map_err(|_| "worker panicked".to_owned())
                .and_then(|outcome| outcome)
        })
        .collect::<Result<Vec<_>, String>>()?;
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == StoredApprovalFinalizationOutcome::Finalized)
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == StoredApprovalFinalizationOutcome::AlreadyConsumed)
            .count(),
        7
    );
    Ok(())
}

#[test]
fn approval_identifier_collision_cannot_rebind_persisted_facts() -> Result<(), Box<dyn Error>> {
    let store = ApplicationStore::open_in_memory()?;
    let original = approval_finalization();
    assert_eq!(
        store.finalize_transaction_approval(&original)?,
        StoredApprovalFinalizationOutcome::Finalized
    );
    let mut changed = original;
    changed.chain_id = "1".to_owned();
    assert_eq!(
        store.finalize_transaction_approval(&changed)?,
        StoredApprovalFinalizationOutcome::IdentifierCollision
    );
    let replay = store.connection.query_row(
        "SELECT outcome, attempted_commitment_digest
             FROM transaction_approval_replay_events",
        [],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?;
    assert_eq!(
        replay,
        ("identifier_collision".to_owned(), changed.commitment_digest)
    );
    Ok(())
}
