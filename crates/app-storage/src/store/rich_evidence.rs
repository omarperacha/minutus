//! Persistence for untrusted rich-evidence checkpoints.

use agent_core::RichEvidenceCheckpoint;
use rusqlite::{OptionalExtension, params};

use super::ApplicationStore;
use crate::StorageError;

pub(super) fn save(
    store: &mut ApplicationStore,
    checkpoint: &RichEvidenceCheckpoint,
) -> Result<(), StorageError> {
    checkpoint
        .validate()
        .map_err(|_| StorageError::InvalidRecord("rich evidence checkpoint is invalid"))?;
    let encoded = serde_json::to_string(checkpoint)?;
    store.connection.execute(
        "INSERT INTO rich_evidence_checkpoints
            (conversation_id, project_id, covered_message_count, covered_message_id,
             history_sha256, source_state_sha256, extraction_policy_version,
             binding_sha256, checkpoint_json, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(conversation_id) DO UPDATE SET
            project_id = excluded.project_id,
            covered_message_count = excluded.covered_message_count,
            covered_message_id = excluded.covered_message_id,
            history_sha256 = excluded.history_sha256,
            source_state_sha256 = excluded.source_state_sha256,
            extraction_policy_version = excluded.extraction_policy_version,
            binding_sha256 = excluded.binding_sha256,
            checkpoint_json = excluded.checkpoint_json,
            updated_at = excluded.updated_at",
        params![
            checkpoint.conversation_id(),
            checkpoint.project_id(),
            checkpoint.covered_message_count(),
            checkpoint.covered_message_id(),
            checkpoint.history_sha256(),
            checkpoint.source_state_sha256(),
            checkpoint.extraction_policy_version(),
            checkpoint.binding_sha256(),
            encoded,
            checkpoint.updated_at(),
        ],
    )?;
    Ok(())
}

pub(super) fn load(
    store: &ApplicationStore,
    conversation_id: &str,
    project_id: &str,
) -> Result<Option<RichEvidenceCheckpoint>, StorageError> {
    validate_scope(conversation_id, project_id)?;
    let row = store
        .connection
        .query_row(
            "SELECT covered_message_count, covered_message_id, history_sha256,
                source_state_sha256, extraction_policy_version, binding_sha256,
                checkpoint_json, updated_at
         FROM rich_evidence_checkpoints
         WHERE conversation_id = ?1 AND project_id = ?2",
            params![conversation_id, project_id],
            |row| {
                Ok((
                    row.get::<_, u32>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, u16>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                ))
            },
        )
        .optional()?;
    row.map(
        |(count, id, history, source_state, policy, binding, encoded, updated_at)| {
            let checkpoint: RichEvidenceCheckpoint = serde_json::from_str(&encoded)?;
            checkpoint.validate().map_err(|_| {
                StorageError::InvalidRecord("stored rich evidence checkpoint is invalid")
            })?;
            if checkpoint.conversation_id() != conversation_id
                || checkpoint.project_id() != project_id
                || checkpoint.covered_message_count() != count
                || checkpoint.covered_message_id() != id
                || checkpoint.history_sha256() != history
                || checkpoint.source_state_sha256() != source_state
                || checkpoint.extraction_policy_version() != policy
                || checkpoint.binding_sha256() != binding
                || checkpoint.updated_at() != updated_at
            {
                return Err(StorageError::InvalidRecord(
                    "rich evidence checkpoint metadata does not match its record",
                ));
            }
            Ok(checkpoint)
        },
    )
    .transpose()
}

pub(super) fn delete(
    store: &mut ApplicationStore,
    conversation_id: &str,
    project_id: &str,
) -> Result<bool, StorageError> {
    validate_scope(conversation_id, project_id)?;
    Ok(store.connection.execute(
        "DELETE FROM rich_evidence_checkpoints
         WHERE conversation_id = ?1 AND project_id = ?2",
        params![conversation_id, project_id],
    )? == 1)
}

fn validate_scope(conversation_id: &str, project_id: &str) -> Result<(), StorageError> {
    if [conversation_id, project_id].iter().any(|value| {
        value.is_empty() || value.len() > 160 || value.chars().any(char::is_whitespace)
    }) {
        return Err(StorageError::InvalidRecord(
            "rich evidence checkpoint scope is invalid",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
