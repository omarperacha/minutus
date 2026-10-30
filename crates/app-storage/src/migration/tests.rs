use super::*;

#[test]
fn version_three_grants_migrate_to_read_only() -> Result<(), Box<dyn std::error::Error>> {
    let mut connection = Connection::open_in_memory()?;
    connection.execute_batch(MIGRATION_1)?;
    connection.execute_batch(MIGRATION_2)?;
    connection.execute_batch(MIGRATION_3)?;
    connection.pragma_update(None, "application_id", APPLICATION_ID)?;
    connection.pragma_update(None, "user_version", 3)?;
    connection.execute(
        "INSERT INTO workspace_grants
                (id, display_name, canonical_root, bookmark_data, created_at, revoked_at)
             VALUES ('grant-1', 'Project', '/Project', X'01', '2026-08-16T12:00:00Z', NULL)",
        [],
    )?;

    initialize(&mut connection)?;

    let permission: String = connection.query_row(
        "SELECT permission_level FROM workspace_grants WHERE id = 'grant-1'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(permission, "read_only");
    assert_eq!(pragma_u32(&connection, "user_version")?, SCHEMA_VERSION);
    Ok(())
}

#[test]
fn version_four_data_migrates_into_one_project() -> Result<(), Box<dyn std::error::Error>> {
    let mut connection = Connection::open_in_memory()?;
    for migration in [MIGRATION_1, MIGRATION_2, MIGRATION_3, MIGRATION_4] {
        connection.execute_batch(migration)?;
    }
    connection.pragma_update(None, "application_id", APPLICATION_ID)?;
    connection.pragma_update(None, "user_version", 4)?;
    connection.execute(
        "INSERT INTO conversations (id, title, created_at, updated_at)
             VALUES ('chat-1', 'Existing chat', '2026-08-16T11:00:00Z', '2026-08-16T12:00:00Z')",
        [],
    )?;
    for (id, name, created_at) in [
        ("grant-1", "Primary", "2026-08-16T10:00:00Z"),
        ("grant-2", "Extra", "2026-08-16T10:30:00Z"),
    ] {
        connection.execute(
                "INSERT INTO workspace_grants
                    (id, display_name, canonical_root, bookmark_data, permission_level, created_at, revoked_at)
                 VALUES (?1, ?2, ?3, X'01', 'read_only', ?4, NULL)",
                rusqlite::params![id, name, format!("/{name}"), created_at],
            )?;
    }

    initialize(&mut connection)?;

    let project_name: String = connection.query_row(
        "SELECT name FROM projects WHERE id = 'project-default'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(project_name, "Primary");
    let chat_project: String = connection.query_row(
        "SELECT project_id FROM conversations WHERE id = 'chat-1'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(chat_project, "project-default");
    let roles = connection
        .prepare(
            "SELECT role FROM project_workspaces
                 WHERE project_id = 'project-default' ORDER BY workspace_grant_id",
        )?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(roles, vec!["primary", "additional"]);
    Ok(())
}

#[test]
fn version_five_audit_rows_gain_safe_defaults() -> Result<(), Box<dyn std::error::Error>> {
    let mut connection = Connection::open_in_memory()?;
    for migration in [
        MIGRATION_1,
        MIGRATION_2,
        MIGRATION_3,
        MIGRATION_4,
        MIGRATION_5,
    ] {
        connection.execute_batch(migration)?;
    }
    connection.pragma_update(None, "application_id", APPLICATION_ID)?;
    connection.pragma_update(None, "user_version", 5)?;
    connection.execute(
        "INSERT INTO audit_events
                (id, event_kind, outcome, subject_id, occurred_at)
             VALUES ('event-1', 'workspace_granted', 'succeeded', 'grant-1',
                     '2026-08-16T12:00:00Z')",
        [],
    )?;

    initialize(&mut connection)?;

    let row = connection.query_row(
        "SELECT event_kind, permission_decision, tool_name, relative_path
             FROM audit_events WHERE id = 'event-1'",
        [],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        },
    )?;
    assert_eq!(
        row,
        (
            "workspace_granted".to_owned(),
            "not_applicable".to_owned(),
            None,
            None
        )
    );
    assert_eq!(pragma_u32(&connection, "user_version")?, SCHEMA_VERSION);
    Ok(())
}

#[test]
fn version_six_audit_rows_gain_image_inspection_tool() -> Result<(), Box<dyn std::error::Error>> {
    let mut connection = Connection::open_in_memory()?;
    for migration in [
        MIGRATION_1,
        MIGRATION_2,
        MIGRATION_3,
        MIGRATION_4,
        MIGRATION_5,
        MIGRATION_6,
    ] {
        connection.execute_batch(migration)?;
    }
    connection.pragma_update(None, "application_id", APPLICATION_ID)?;
    connection.pragma_update(None, "user_version", 6)?;
    connection.execute(
        "INSERT INTO audit_events (
                id, event_kind, outcome, operation_id, tool_name,
                workspace_grant_id, relative_path, permission_decision, occurred_at
             ) VALUES (
                'event-1', 'filesystem_tool', 'succeeded', 'operation-1',
                'filesystem.read_text', 'grant-1', 'README.md', 'allowed',
                '2026-08-17T10:00:00Z'
             )",
        [],
    )?;

    initialize(&mut connection)?;

    connection.execute(
        "INSERT INTO audit_events (
                id, event_kind, outcome, operation_id, tool_name,
                workspace_grant_id, relative_path, permission_decision, occurred_at
             ) VALUES (
                'event-2', 'filesystem_tool', 'succeeded', 'operation-2',
                'filesystem.inspect_image', 'grant-1', 'diagram.png', 'allowed',
                '2026-08-17T10:01:00Z'
             )",
        [],
    )?;
    let count: i64 =
        connection.query_row("SELECT count(*) FROM audit_events", [], |row| row.get(0))?;
    assert_eq!(count, 2);
    assert_eq!(pragma_u32(&connection, "user_version")?, SCHEMA_VERSION);
    Ok(())
}

#[test]
fn version_ten_audit_rows_gain_path_discovery_tool() -> Result<(), Box<dyn std::error::Error>> {
    let mut connection = Connection::open_in_memory()?;
    for migration in &MIGRATIONS[..10] {
        connection.execute_batch(migration)?;
    }
    connection.pragma_update(None, "application_id", APPLICATION_ID)?;
    connection.pragma_update(None, "user_version", 10)?;

    initialize(&mut connection)?;
    connection.execute(
        "INSERT INTO audit_events (
                id, event_kind, outcome, operation_id, tool_name,
                workspace_grant_id, permission_decision, result_count,
                truncated, occurred_at
             ) VALUES (
                'event-find', 'filesystem_tool', 'succeeded', 'operation-find',
                'filesystem.find_paths', 'grant-find', 'allowed', 2, 0,
                '2026-08-18T12:00:00Z'
             )",
        [],
    )?;
    let tool: String = connection.query_row(
        "SELECT tool_name FROM audit_events WHERE id = 'event-find'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(tool, "filesystem.find_paths");
    assert_eq!(pragma_u32(&connection, "user_version")?, SCHEMA_VERSION);
    Ok(())
}

#[test]
fn version_eleven_conversations_upgrade_with_unbound_model_identity()
-> Result<(), Box<dyn std::error::Error>> {
    let mut connection = Connection::open_in_memory()?;
    for migration in &MIGRATIONS[..11] {
        connection.execute_batch(migration)?;
    }
    connection.pragma_update(None, "application_id", APPLICATION_ID)?;
    connection.pragma_update(None, "user_version", 11)?;
    connection.execute(
        "INSERT INTO projects (id, name, created_at, updated_at)
             VALUES ('project-1', 'Project', '2026-08-20T00:00:00Z', '2026-08-20T00:00:00Z')",
        [],
    )?;
    connection.execute(
            "INSERT INTO conversations (id, title, created_at, updated_at, project_id)
             VALUES ('chat-1', 'Existing', '2026-08-20T00:00:00Z', '2026-08-20T00:00:00Z', 'project-1')",
            [],
        )?;

    initialize(&mut connection)?;
    let pair = connection.query_row(
        "SELECT model_id, model_version FROM conversations WHERE id = 'chat-1'",
        [],
        |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
            ))
        },
    )?;
    assert_eq!(pair, (None, None));
    assert_eq!(pragma_u32(&connection, "user_version")?, SCHEMA_VERSION);
    Ok(())
}

#[test]
fn version_twelve_projects_upgrade_with_unbound_model_identity()
-> Result<(), Box<dyn std::error::Error>> {
    let mut connection = Connection::open_in_memory()?;
    for migration in &MIGRATIONS[..12] {
        connection.execute_batch(migration)?;
    }
    connection.pragma_update(None, "application_id", APPLICATION_ID)?;
    connection.pragma_update(None, "user_version", 12)?;
    connection.execute(
        "INSERT INTO projects (id, name, created_at, updated_at)
             VALUES ('project-1', 'Project', '2026-08-23T00:00:00Z', '2026-08-23T00:00:00Z')",
        [],
    )?;

    initialize(&mut connection)?;
    let pair = connection.query_row(
        "SELECT model_id, model_version FROM projects WHERE id = 'project-1'",
        [],
        |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
            ))
        },
    )?;
    assert_eq!(pair, (None, None));
    assert_eq!(pragma_u32(&connection, "user_version")?, SCHEMA_VERSION);
    Ok(())
}

#[test]
fn version_fourteen_adds_restart_safe_evm_monitoring() -> Result<(), Box<dyn std::error::Error>> {
    let mut connection = Connection::open_in_memory()?;
    for migration in &MIGRATIONS[..14] {
        connection.execute_batch(migration)?;
    }
    connection.pragma_update(None, "application_id", APPLICATION_ID)?;
    connection.pragma_update(None, "user_version", 14)?;

    initialize(&mut connection)?;
    connection.execute(
        "INSERT INTO evm_transactions (
                transaction_hash, network, sender, nonce, candidate_digest,
                state, submitted_at, updated_at
             ) VALUES (?1, 'eip155:8453', ?2, '0', ?3, 'pending', ?4, ?4)",
        rusqlite::params![
            format!("0x{}", "11".repeat(32)),
            "0x9858effd232b4033e47d90003d41ec34ecaeda94",
            format!("0x{}", "22".repeat(32)),
            "2026-09-01T00:00:00Z",
        ],
    )?;
    assert_eq!(pragma_u32(&connection, "user_version")?, SCHEMA_VERSION);
    Ok(())
}

#[test]
fn version_sixteen_adds_durable_single_use_approvals() -> Result<(), Box<dyn std::error::Error>> {
    let mut connection = Connection::open_in_memory()?;
    for migration in &MIGRATIONS[..16] {
        connection.execute_batch(migration)?;
    }
    connection.pragma_update(None, "application_id", APPLICATION_ID)?;
    connection.pragma_update(None, "user_version", 16)?;

    initialize(&mut connection)?;
    let table_count: i64 = connection.query_row(
        "SELECT count(*) FROM sqlite_schema
             WHERE type = 'table' AND name IN (
                'transaction_approval_finalizations',
                'transaction_approval_replay_events'
             )",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(table_count, 2);
    assert_eq!(pragma_u32(&connection, "user_version")?, SCHEMA_VERSION);
    Ok(())
}

#[test]
fn version_seventeen_adds_empty_continuity_without_inventing_history()
-> Result<(), Box<dyn std::error::Error>> {
    let mut connection = Connection::open_in_memory()?;
    for migration in &MIGRATIONS[..17] {
        connection.execute_batch(migration)?;
    }
    connection.pragma_update(None, "application_id", APPLICATION_ID)?;
    connection.pragma_update(None, "user_version", 17)?;
    initialize(&mut connection)?;
    assert_eq!(pragma_u32(&connection, "user_version")?, SCHEMA_VERSION);
    let count: u32 = connection.query_row(
        "SELECT count(*) FROM dialogue_continuity_checkpoints",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(count, 0);
    Ok(())
}

#[test]
fn version_eighteen_upgrade_preserves_managed_installs_without_inventing_external_registrations()
-> Result<(), Box<dyn std::error::Error>> {
    let mut connection = Connection::open_in_memory()?;
    for migration in &MIGRATIONS[..18] {
        connection.execute_batch(migration)?;
    }
    connection.pragma_update(None, "application_id", APPLICATION_ID)?;
    connection.pragma_update(None, "user_version", 18)?;
    connection.execute("INSERT INTO models (id, display_name, format, local_path, content_digest, byte_size, imported_at) VALUES ('managed', 'Managed model', 'gguf', '/managed/weights.gguf', ?1, 20, '2026-10-20T00:00:00Z')", ["a".repeat(64)])?;
    initialize(&mut connection)?;
    let path: String = connection.query_row(
        "SELECT local_path FROM models WHERE id = 'managed'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(path, "/managed/weights.gguf");
    let registrations: u32 = connection.query_row(
        "SELECT count(*) FROM external_model_registrations",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(registrations, 0);
    Ok(())
}

fn legacy_evm_database(
    path: &std::path::Path,
    version: usize,
) -> Result<Connection, Box<dyn std::error::Error>> {
    let connection = Connection::open(path)?;
    connection.execute_batch("PRAGMA foreign_keys=ON;")?;
    for sql in &MIGRATIONS[..version] {
        connection.execute_batch(sql)?;
    }
    connection.pragma_update(None, "application_id", APPLICATION_ID)?;
    connection.pragma_update(None, "user_version", u32::try_from(version)?)?;
    Ok(connection)
}
fn legacy_transaction(
    connection: &Connection,
    byte: u8,
    nonce: &str,
    digest: u8,
    phase: Option<&str>,
) -> Result<String, Box<dyn std::error::Error>> {
    let hash = format!("0x{}", format!("{byte:02x}").repeat(32));
    connection.execute("INSERT INTO evm_transactions(transaction_hash,network,sender,nonce,candidate_digest,state,submitted_at,updated_at) VALUES (?1,'eip155:8453','0xabcdefabcdefabcdefabcdefabcdefabcdefabcd',?2,?3,'pending','2026-10-30T00:00:00Z','2026-10-30T00:00:00Z')",rusqlite::params![hash,nonce,format!("0x{}",format!("{digest:02x}").repeat(32))])?;
    if let Some(phase) = phase {
        connection.execute("INSERT INTO evm_transfer_dispatches(transaction_hash,reservation_id,phase) VALUES (?1,'old-worker',?2)",rusqlite::params![hash,phase])?;
    }
    Ok(hash)
}
#[test]
fn schema_twenty_legacy_monitoring_backfills_blocking_nonce_without_invented_dispatch()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("legacy.sqlite");
    let legacy = legacy_evm_database(&path, 20)?;
    let hash = legacy_transaction(&legacy, 1, "7", 0x22, None)?;
    let mut store = crate::ApplicationStore::open(&path)?;
    assert_eq!(store.evm_transactions()?.len(), 1);
    assert_eq!(store.evm_dispatch_phase(&hash)?, None);
    assert_eq!(
        store
            .retire_evm_reservations("eip155:8453", "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd")?,
        0
    );
    assert!(store.acknowledge_evm_uncertainty(&hash)?);
    assert!(
        store
            .claim_evm_nonce(
                "eip155:8453",
                "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd",
                "7",
                &format!("0x{}", "22".repeat(32)),
                "new"
            )?
            .is_none()
    );
    Ok(())
}
#[test]
fn schema_twenty_two_duplicate_hashes_remain_blocked_and_old_open_workers_are_fenced()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("legacy.sqlite");
    let legacy = legacy_evm_database(&path, 22)?;
    let first = legacy_transaction(&legacy, 1, "7", 0x22, Some("dispatched"))?;
    let second = legacy_transaction(&legacy, 2, "7", 0x33, Some("acknowledged"))?;
    let unsent = legacy_transaction(&legacy, 3, "8", 0x44, Some("reserved"))?;
    let mut store = crate::ApplicationStore::open(&path)?;
    assert_eq!(store.evm_transactions()?.len(), 3);
    assert_eq!(
        legacy.query_row(
            "SELECT count(*) FROM evm_nonce_claims WHERE nonce='7' AND phase='possible_send'",
            [],
            |row| row.get::<_, u32>(0)
        )?,
        1
    );
    assert_eq!(
        legacy.query_row(
            "SELECT count(*) FROM evm_nonce_hashes WHERE transaction_hash IN (?1,?2)",
            rusqlite::params![first, second],
            |row| row.get::<_, u32>(0)
        )?,
        2
    );
    assert!(
        store
            .claim_evm_nonce(
                "eip155:8453",
                "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd",
                "7",
                &format!("0x{}", "22".repeat(32)),
                "new"
            )?
            .is_none()
    );
    assert!(
        legacy
            .execute(
                "UPDATE evm_transfer_dispatches SET phase='dispatched' WHERE transaction_hash=?1",
                [&unsent]
            )
            .is_err()
    );
    assert!(legacy.execute("UPDATE evm_transfer_dispatches SET reservation_id='new-worker' WHERE transaction_hash=?1",[&unsent]).is_err());
    assert!(
        legacy
            .execute(
                "UPDATE evm_transfer_dispatches SET phase='acknowledged' WHERE transaction_hash=?1",
                [&unsent]
            )
            .is_err()
    );
    assert!(legacy.execute("INSERT INTO evm_transfer_dispatches(transaction_hash,reservation_id,phase) VALUES (?1,'old-worker','dispatched')",[&unsent]).is_err());
    assert_eq!(
        store
            .retire_evm_reservations("eip155:8453", "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd")?,
        1
    );
    assert!(
        store
            .claim_evm_nonce(
                "eip155:8453",
                "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd",
                "8",
                &format!("0x{}", "44".repeat(32)),
                "new"
            )?
            .is_some()
    );
    Ok(())
}

#[test]
fn missing_nonce_fences_fail_schema_validation() -> Result<(), Box<dyn std::error::Error>> {
    for name in REQUIRED_NONCE_TRIGGERS {
        let mut connection = Connection::open_in_memory()?;
        initialize(&mut connection)?;
        connection.execute_batch(&format!("DROP TRIGGER {name}"))?;
        assert!(matches!(
            validate(&connection),
            Err(StorageError::CorruptDatabase)
        ));
    }
    let mut connection = Connection::open_in_memory()?;
    initialize(&mut connection)?;
    connection.execute_batch("DROP INDEX evm_nonce_active_key")?;
    assert!(matches!(
        validate(&connection),
        Err(StorageError::CorruptDatabase)
    ));
    Ok(())
}
