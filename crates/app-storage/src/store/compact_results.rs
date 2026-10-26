//! Persistence and retention for host-only compact tool results.

use agent_core::{CompactResultStatus, CompactToolResultRecord, HostSourceBinding};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

use super::{ApplicationStore, query_unreferenced_assets};
use crate::{AssetReference, StorageError};

const MAX_SERIALIZED_RECORD_BYTES: usize = 17_825_792;

pub(super) fn save(
    store: &mut ApplicationStore,
    record: &CompactToolResultRecord,
) -> Result<(), StorageError> {
    record
        .validate()
        .map_err(|_| StorageError::InvalidRecord("compact tool result is invalid"))?;
    let serialized = serde_json::to_string(record)?;
    if serialized.len() > MAX_SERIALIZED_RECORD_BYTES {
        return Err(StorageError::InvalidRecord(
            "compact tool result is too large",
        ));
    }
    let transaction = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    let scope_exists = transaction.query_row(
        "SELECT EXISTS (
            SELECT 1 FROM conversations
            WHERE id = ?1 AND project_id = ?2
         )",
        params![record.conversation_id(), record.project_id()],
        |row| row.get::<_, bool>(0),
    )?;
    if !scope_exists {
        return Err(StorageError::InvalidRecord(
            "compact tool result scope does not exist",
        ));
    }
    validate_managed_assets(&transaction, record)?;
    transaction.execute(
        "INSERT INTO compact_tool_results
            (id, conversation_id, project_id, tool_call_id, tool_name,
             status, record_json, created_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            record.id(),
            record.conversation_id(),
            record.project_id(),
            record.tool_call_id(),
            record.tool_name(),
            status_name(record.status()),
            serialized,
            record.created_at(),
            record.expires_at(),
        ],
    )?;
    for asset_id in record.asset_ids() {
        transaction.execute(
            "INSERT INTO compact_result_assets (compact_result_id, asset_id)
             VALUES (?1, ?2)",
            params![record.id(), asset_id],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

pub(super) fn load(
    store: &ApplicationStore,
    id: &str,
    conversation_id: &str,
    project_id: &str,
) -> Result<Option<CompactToolResultRecord>, StorageError> {
    validate_lookup_scope(id, conversation_id, project_id)?;
    let stored = store
        .connection
        .query_row(
            "SELECT tool_call_id, tool_name, status, record_json, created_at, expires_at
             FROM compact_tool_results
             WHERE id = ?1 AND conversation_id = ?2 AND project_id = ?3",
            params![id, conversation_id, project_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()?;
    let record = stored
        .map(
            |(tool_call_id, tool_name, status, serialized, created_at, expires_at)| {
                let record: CompactToolResultRecord = serde_json::from_str(&serialized)?;
                record.validate().map_err(|_| {
                    StorageError::InvalidRecord("stored compact tool result is invalid")
                })?;
                if record.id() != id
                    || record.conversation_id() != conversation_id
                    || record.project_id() != project_id
                    || record.tool_call_id() != tool_call_id
                    || record.tool_name() != tool_name
                    || status_name(record.status()) != status
                    || record.created_at() != created_at
                    || record.expires_at() != expires_at
                {
                    return Err(StorageError::InvalidRecord(
                        "compact tool result metadata does not match its record",
                    ));
                }
                Ok(record)
            },
        )
        .transpose()?;
    if let Some(record) = &record {
        validate_asset_associations(store, record)?;
        validate_managed_assets(&store.connection, record)?;
    }
    Ok(record)
}

pub(super) fn reap_expired(
    store: &mut ApplicationStore,
    now: &str,
) -> Result<(u64, Vec<AssetReference>), StorageError> {
    if !valid_retention_timestamp(now) {
        return Err(StorageError::InvalidRecord(
            "compact result retention timestamp is invalid",
        ));
    }
    let transaction = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    let deleted = transaction.execute(
        "DELETE FROM compact_tool_results WHERE expires_at <= ?1",
        [now],
    )?;
    let assets = query_unreferenced_assets(&transaction)?;
    transaction.execute(
        "DELETE FROM assets
         WHERE NOT EXISTS (
            SELECT 1 FROM message_assets WHERE message_assets.asset_id = assets.id
         ) AND NOT EXISTS (
            SELECT 1 FROM compact_result_assets
            WHERE compact_result_assets.asset_id = assets.id
         )",
        [],
    )?;
    transaction.commit()?;
    let deleted = u64::try_from(deleted)
        .map_err(|_| StorageError::InvalidRecord("compact result count is invalid"))?;
    Ok((deleted, assets))
}

fn validate_managed_assets(
    transaction: &rusqlite::Connection,
    record: &CompactToolResultRecord,
) -> Result<(), StorageError> {
    for source in record.source_bindings() {
        let HostSourceBinding::ManagedAsset {
            asset_id,
            content_sha256,
            ..
        } = source
        else {
            continue;
        };
        let stored_digest = transaction
            .query_row(
                "SELECT content_digest FROM assets WHERE id = ?1",
                [asset_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if stored_digest.as_deref() != Some(content_sha256.as_str()) {
            return Err(StorageError::InvalidRecord(
                "compact tool result asset is missing or changed",
            ));
        }
    }
    Ok(())
}

fn validate_asset_associations(
    store: &ApplicationStore,
    record: &CompactToolResultRecord,
) -> Result<(), StorageError> {
    let mut statement = store.connection.prepare(
        "SELECT asset_id FROM compact_result_assets
         WHERE compact_result_id = ?1 ORDER BY asset_id",
    )?;
    let associated = statement
        .query_map([record.id()], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let expected = record
        .asset_ids()
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if associated != expected {
        return Err(StorageError::InvalidRecord(
            "compact tool result asset associations are corrupt",
        ));
    }
    Ok(())
}

fn validate_lookup_scope(
    id: &str,
    conversation_id: &str,
    project_id: &str,
) -> Result<(), StorageError> {
    if [id, conversation_id, project_id].iter().any(|value| {
        value.is_empty() || value.len() > 160 || value.chars().any(char::is_whitespace)
    }) {
        return Err(StorageError::InvalidRecord(
            "compact tool result lookup scope is invalid",
        ));
    }
    Ok(())
}

const fn status_name(status: CompactResultStatus) -> &'static str {
    match status {
        CompactResultStatus::Completed => "completed",
        CompactResultStatus::Failed => "failed",
        CompactResultStatus::Cancelled => "cancelled",
    }
}

fn valid_retention_timestamp(value: &str) -> bool {
    let bytes = value.as_bytes();
    value.len() == 20
        && bytes.get(4) == Some(&b'-')
        && bytes.get(7) == Some(&b'-')
        && bytes.get(10) == Some(&b'T')
        && bytes.get(13) == Some(&b':')
        && bytes.get(16) == Some(&b':')
        && bytes.get(19) == Some(&b'Z')
        && bytes.iter().enumerate().all(|(index, byte)| {
            matches!(index, 4 | 7 | 10 | 13 | 16 | 19) || byte.is_ascii_digit()
        })
}

#[cfg(test)]
mod tests;
