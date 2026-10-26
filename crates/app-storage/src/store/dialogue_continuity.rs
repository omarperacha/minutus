//! Persisted dialogue boundaries with explicit compare-and-swap advancement.

use super::ApplicationStore;
use crate::StorageError;
use agent_core::DialogueContinuityCheckpoint;
use rusqlite::{OptionalExtension, params};

impl ApplicationStore {
    /// Reads a validated checkpoint in its exact conversation/project scope.
    ///
    /// # Errors
    /// Returns an error for malformed stored data or a database failure.
    pub fn dialogue_continuity_checkpoint(
        &self,
        conversation_id: &str,
        project_id: &str,
    ) -> Result<Option<DialogueContinuityCheckpoint>, StorageError> {
        let encoded: Option<String> = self.connection.query_row(
            "SELECT checkpoint_json FROM dialogue_continuity_checkpoints WHERE conversation_id = ?1 AND project_id = ?2",
            params![conversation_id, project_id], |row| row.get(0)).optional()?;
        encoded
            .map(|encoded| {
                let checkpoint: DialogueContinuityCheckpoint = serde_json::from_str(&encoded)?;
                checkpoint
                    .validate()
                    .map_err(|_| StorageError::InvalidRecord("invalid dialogue checkpoint"))?;
                if checkpoint.conversation_id() != conversation_id
                    || checkpoint.project_id() != project_id
                {
                    return Err(StorageError::InvalidRecord(
                        "dialogue checkpoint scope mismatch",
                    ));
                }
                Ok(checkpoint)
            })
            .transpose()
    }

    /// Advances a boundary only if the previously admitted checkpoint still owns it.
    ///
    /// # Errors
    /// Returns an error for scope changes, non-forward movement or invalid data.
    pub fn advance_dialogue_continuity(
        &mut self,
        previous: &DialogueContinuityCheckpoint,
        next: &DialogueContinuityCheckpoint,
    ) -> Result<bool, StorageError> {
        for checkpoint in [previous, next] {
            checkpoint
                .validate()
                .map_err(|_| StorageError::InvalidRecord("invalid dialogue checkpoint"))?;
        }
        if previous.conversation_id() != next.conversation_id()
            || previous.project_id() != next.project_id()
            || previous.retained_start() >= next.retained_start()
        {
            return Err(StorageError::InvalidRecord("invalid dialogue advancement"));
        }
        Ok(self.connection.execute(
            "UPDATE dialogue_continuity_checkpoints SET checkpoint_json = ?1
             WHERE conversation_id = ?2 AND project_id = ?3 AND checkpoint_json = ?4",
            params![
                serde_json::to_string(next)?,
                next.conversation_id(),
                next.project_id(),
                serde_json::to_string(previous)?
            ],
        )? == 1)
    }

    /// Installs an initial boundary without replacing an existing boundary.
    ///
    /// # Errors
    /// Returns an error for invalid data, scope mismatch or a database failure.
    pub fn initialise_dialogue_continuity(
        &mut self,
        checkpoint: &DialogueContinuityCheckpoint,
    ) -> Result<(), StorageError> {
        checkpoint
            .validate()
            .map_err(|_| StorageError::InvalidRecord("invalid dialogue checkpoint"))?;
        self.connection.execute(
            "INSERT INTO dialogue_continuity_checkpoints (conversation_id, project_id, checkpoint_json)
             VALUES (?1, ?2, ?3) ON CONFLICT(conversation_id) DO NOTHING",
            params![checkpoint.conversation_id(), checkpoint.project_id(), serde_json::to_string(checkpoint)?])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
