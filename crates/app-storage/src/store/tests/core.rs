use super::*;

#[test]
fn migration_is_versioned_and_reopens_cleanly() -> Result<(), Box<dyn Error>> {
    let database = NamedTempFile::new()?;
    drop(ApplicationStore::open(database.path())?);
    let store = ApplicationStore::open(database.path())?;
    assert_eq!(store.schema_version()?, SCHEMA_VERSION);
    let migration_count: i64 =
        store
            .connection
            .query_row("SELECT count(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })?;
    assert_eq!(migration_count, i64::from(SCHEMA_VERSION));
    Ok(())
}

#[test]
fn project_model_preference_round_trips_and_clears() -> Result<(), Box<dyn Error>> {
    let database = NamedTempFile::new()?;
    let mut store = ApplicationStore::open(database.path())?;
    save_test_project(&store)?;
    let project_id = ProjectId("project-default".to_owned());
    let selection = ModelSelection {
        model_id: "local/model".to_owned(),
        model_version: "1.0".to_owned(),
    };

    assert_eq!(store.project_model(&project_id)?, None);
    assert!(store.set_project_model(&project_id, Some(&selection), "2026-08-14T12:01:00Z",)?);
    assert_eq!(store.project_model(&project_id)?, Some(selection));
    assert!(store.set_project_model(&project_id, None, "2026-08-14T12:02:00Z")?);
    assert_eq!(store.project_model(&project_id)?, None);
    Ok(())
}

#[test]
fn execution_ledger_survives_restart_and_follows_conversation_lifecycle()
-> Result<(), Box<dyn Error>> {
    let database = NamedTempFile::new()?;
    let conversation_id = ConversationId("conversation-1".to_owned());
    let snapshot = execution_ledger_snapshot()?;
    {
        let mut store = ApplicationStore::open(database.path())?;
        save_test_project(&store)?;
        save_test_conversation(&mut store, &conversation_id)?;
        store.save_execution_ledger(&snapshot, NOW)?;
    }

    let mut reopened = ApplicationStore::open(database.path())?;
    let restored_snapshot = reopened
        .execution_ledger("run-1")?
        .ok_or("persisted execution ledger missing")?;
    let restored = ExecutionLedger::restore(restored_snapshot)?;
    let mutation = restored
        .authoritative_mutation("primary", "src/lib.rs")
        .ok_or("authoritative mutation missing after restart")?;
    assert_eq!(mutation.resulting_sha256(), "b".repeat(64));
    assert_eq!(mutation.resource_revision(), "resource-r2");

    let (deleted, _) = reopened.delete_conversation_and_reap_assets(&conversation_id)?;
    assert!(deleted);
    assert!(reopened.execution_ledger("run-1")?.is_none());
    Ok(())
}

#[test]
fn execution_ledger_rejects_missing_or_changed_scope() -> Result<(), Box<dyn Error>> {
    let mut store = ApplicationStore::open_in_memory()?;
    save_test_project(&store)?;
    let snapshot = execution_ledger_snapshot()?;
    assert!(matches!(
        store.save_execution_ledger(&snapshot, NOW),
        Err(StorageError::InvalidRecord(_))
    ));

    let conversation_id = ConversationId("conversation-1".to_owned());
    save_test_conversation(&mut store, &conversation_id)?;
    store.save_execution_ledger(&snapshot, NOW)?;
    store.save_project(&Project {
        id: ProjectId("project-other".to_owned()),
        name: "Other project".to_owned(),
        created_at: NOW.to_owned(),
        updated_at: NOW.to_owned(),
    })?;
    let mut moved = store
        .conversation(&conversation_id)?
        .ok_or("conversation missing")?;
    moved.project_id = ProjectId("project-other".to_owned());
    assert!(store.save_conversation(&moved).is_err());
    assert!(store.execution_ledger("run-1")?.is_some());
    Ok(())
}

#[test]
fn future_schema_is_rejected_without_migration() -> Result<(), Box<dyn Error>> {
    let database = NamedTempFile::new()?;
    let store = ApplicationStore::open(database.path())?;
    store
        .connection
        .pragma_update(None, "user_version", SCHEMA_VERSION + 1)?;
    drop(store);
    let result = ApplicationStore::open(database.path());
    assert!(matches!(
        result,
        Err(StorageError::IncompatibleSchema { .. })
    ));
    Ok(())
}

#[test]
fn unrelated_database_is_not_adopted() -> Result<(), Box<dyn Error>> {
    let database = NamedTempFile::new()?;
    let connection = Connection::open(database.path())?;
    connection.execute("CREATE TABLE unrelated (value TEXT)", [])?;
    drop(connection);
    assert!(matches!(
        ApplicationStore::open(database.path()),
        Err(StorageError::ForeignDatabase)
    ));
    Ok(())
}

#[test]
fn non_database_bytes_fail_without_panicking() -> Result<(), Box<dyn Error>> {
    let database = NamedTempFile::new()?;
    fs::write(database.path(), b"this is not sqlite")?;
    assert!(ApplicationStore::open(database.path()).is_err());
    Ok(())
}

#[test]
fn typed_settings_round_trip_and_mismatches_fail_closed() -> Result<(), Box<dyn Error>> {
    let store = ApplicationStore::open_in_memory()?;
    let value = SettingValue::Theme(Theme::Dark);
    store.set_setting(&value, NOW)?;
    assert_eq!(store.setting(SettingKey::Theme)?, Some(value));
    store.connection.execute(
        "UPDATE settings SET value_json = ?1 WHERE key = 'theme'",
        [serde_json::to_string(&SettingValue::ReduceMotion(true))?],
    )?;
    assert!(matches!(
        store.setting(SettingKey::Theme),
        Err(StorageError::InvalidRecord(_))
    ));
    Ok(())
}
