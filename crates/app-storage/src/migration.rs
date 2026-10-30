//! Versioned `SQLite` schema migrations.

use rusqlite::{Connection, TransactionBehavior};

use crate::StorageError;

mod schema;

use schema::{
    MIGRATION_1, MIGRATION_2, MIGRATION_3, MIGRATION_4, MIGRATION_5, MIGRATION_6, MIGRATION_7,
    MIGRATION_8, MIGRATION_9, MIGRATION_10, MIGRATION_11, MIGRATION_12, MIGRATION_13, MIGRATION_14,
    MIGRATION_15, MIGRATION_16, MIGRATION_17, MIGRATION_18, MIGRATION_19, MIGRATION_20,
    MIGRATION_21, MIGRATION_22, MIGRATION_23, MIGRATION_24,
};

/// `SQLite` application identifier for ASCII `ZETA`.
const APPLICATION_ID: u32 = 0x5A45_5441;

/// Latest schema version understood by this build.
pub const SCHEMA_VERSION: u32 = 24;

const REQUIRED_TABLES: [&str; 33] = [
    "schema_migrations",
    "evm_nonce_claims",
    "evm_nonce_hashes",
    "contract_workflow_scopes",
    "contract_workflow_candidates",
    "contract_workflow_observations",
    "contract_workflows",
    "contract_workflow_steps",
    "chat_transfer_attempts",
    "evm_transfer_dispatches",
    "dialogue_continuity_checkpoints",
    "settings",
    "conversations",
    "assets",
    "messages",
    "message_assets",
    "workspace_grants",
    "projects",
    "project_workspaces",
    "models",
    "external_model_registrations",
    "model_companions",
    "public_wallets",
    "audit_events",
    "execution_ledgers",
    "compact_tool_results",
    "compact_result_assets",
    "semantic_compaction_checkpoints",
    "rich_evidence_checkpoints",
    "provider_model_installs",
    "evm_transactions",
    "transaction_approval_finalizations",
    "transaction_approval_replay_events",
];

// Nonce fencing also protects already-open older connections. Missing schema
// guards must not turn a reopen into an apparently healthy hash-only sender.
const REQUIRED_NONCE_TRIGGERS: [&str; 12] = [
    "evm_nonce_hashes_no_delete",
    "evm_nonce_hashes_immutable",
    "evm_nonce_no_delete",
    "evm_nonce_binding_immutable",
    "evm_nonce_no_revive",
    "evm_nonce_signed_hash_immutable",
    "evm_transaction_nonce_conflict",
    "evm_transaction_identity_immutable",
    "evm_transaction_nonce_retention",
    "evm_dispatch_requires_nonce",
    "evm_dispatch_identity_immutable",
    "evm_dispatch_requires_fence",
];

const REQUIRED_WORKFLOW_TRIGGERS: [&str; 4] = [
    "contract_scope_format_immutable",
    "contract_scope_immutable",
    "contract_candidate_immutable",
    "contract_observation_immutable",
];

const MIGRATIONS: [&str; 24] = [
    MIGRATION_1,
    MIGRATION_2,
    MIGRATION_3,
    MIGRATION_4,
    MIGRATION_5,
    MIGRATION_6,
    MIGRATION_7,
    MIGRATION_8,
    MIGRATION_9,
    MIGRATION_10,
    MIGRATION_11,
    MIGRATION_12,
    MIGRATION_13,
    MIGRATION_14,
    MIGRATION_15,
    MIGRATION_16,
    MIGRATION_17,
    MIGRATION_18,
    MIGRATION_19,
    MIGRATION_20,
    MIGRATION_21,
    MIGRATION_22,
    MIGRATION_23,
    MIGRATION_24,
];

pub(crate) fn initialize(connection: &mut Connection) -> Result<(), StorageError> {
    connection.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA trusted_schema = OFF;
         PRAGMA busy_timeout = 5000;",
    )?;

    let application_id = pragma_u32(connection, "application_id")?;
    let version = pragma_u32(connection, "user_version")?;

    if application_id != 0 && application_id != APPLICATION_ID {
        return Err(StorageError::ForeignDatabase);
    }
    if version > SCHEMA_VERSION {
        return Err(StorageError::IncompatibleSchema {
            found: version,
            supported: SCHEMA_VERSION,
        });
    }
    let has_tables = has_user_tables(connection)?;
    if application_id == 0 && (version != 0 || has_tables) {
        return Err(StorageError::ForeignDatabase);
    }
    if application_id == APPLICATION_ID && version == 0 && has_tables {
        return Err(StorageError::CorruptDatabase);
    }

    connection.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;",
    )?;

    for next_version in (version + 1)..=SCHEMA_VERSION {
        let migration_index =
            usize::try_from(next_version - 1).map_err(|_| StorageError::CorruptDatabase)?;
        let migration = MIGRATIONS
            .get(migration_index)
            .ok_or(StorageError::CorruptDatabase)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute_batch(migration)?;
        transaction.pragma_update(None, "application_id", APPLICATION_ID)?;
        transaction.pragma_update(None, "user_version", next_version)?;
        transaction.commit()?;
    }

    validate(connection)
}

fn pragma_u32(connection: &Connection, name: &str) -> Result<u32, StorageError> {
    let value = connection.pragma_query_value(None, name, |row| row.get::<_, i64>(0))?;
    u32::try_from(value).map_err(|_| StorageError::CorruptDatabase)
}

fn has_user_tables(connection: &Connection) -> Result<bool, StorageError> {
    let count: i64 = connection.query_row(
        "SELECT count(*) FROM sqlite_schema
         WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
        [],
        |row| row.get(0),
    )?;
    Ok(count != 0)
}

fn validate(connection: &Connection) -> Result<(), StorageError> {
    if pragma_u32(connection, "application_id")? != APPLICATION_ID
        || pragma_u32(connection, "user_version")? != SCHEMA_VERSION
    {
        return Err(StorageError::CorruptDatabase);
    }

    let integrity: String = connection.pragma_query_value(None, "quick_check", |row| row.get(0))?;
    if integrity != "ok" {
        return Err(StorageError::CorruptDatabase);
    }

    for table in REQUIRED_TABLES {
        let count: i64 = connection.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type = 'table' AND name = ?1",
            [table],
            |row| row.get(0),
        )?;
        if count != 1 {
            return Err(StorageError::CorruptDatabase);
        }
    }
    for name in REQUIRED_NONCE_TRIGGERS
        .into_iter()
        .chain(REQUIRED_WORKFLOW_TRIGGERS)
    {
        let count: i64 = connection.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type='trigger' AND name=?1",
            [name],
            |row| row.get(0),
        )?;
        if count != 1 {
            return Err(StorageError::CorruptDatabase);
        }
    }
    let unique_key:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM pragma_index_list('evm_nonce_claims') WHERE name='evm_nonce_active_key' AND [unique]=1 AND partial=1)",[],|row|row.get(0))?;
    if !unique_key {
        return Err(StorageError::CorruptDatabase);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
