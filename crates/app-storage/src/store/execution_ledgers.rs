use super::{
    ApplicationStore, ExecutionLedger, ExecutionLedgerSnapshot, OptionalExtension, StorageError,
    TransactionBehavior, params,
};

impl ApplicationStore {
    /// Persists one validated trusted execution ledger for its conversation.
    ///
    /// A conversation owns at most one active ledger. Replacing that ledger is
    /// atomic, while an existing run ID cannot be rebound to another scope.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed snapshots, missing or mismatched scope,
    /// invalid timestamps, serialization failures, or database failures.
    pub fn save_execution_ledger(
        &mut self,
        snapshot: &ExecutionLedgerSnapshot,
        updated_at: &str,
    ) -> Result<(), StorageError> {
        if updated_at.is_empty() {
            return Err(StorageError::InvalidRecord(
                "execution ledger timestamp is invalid",
            ));
        }
        ExecutionLedger::restore(snapshot.clone())
            .map_err(|_| StorageError::InvalidRecord("execution ledger snapshot is invalid"))?;
        let serialized = serde_json::to_string(snapshot)?;
        if serialized.len() > 1_048_576 {
            return Err(StorageError::InvalidRecord(
                "execution ledger snapshot is too large",
            ));
        }

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let scope_exists = transaction.query_row(
            "SELECT EXISTS (
                SELECT 1 FROM conversations
                WHERE id = ?1 AND project_id = ?2
             )",
            params![snapshot.conversation_id(), snapshot.project_id()],
            |row| row.get::<_, bool>(0),
        )?;
        if !scope_exists {
            return Err(StorageError::InvalidRecord(
                "execution ledger scope does not exist",
            ));
        }

        transaction.execute(
            "DELETE FROM execution_ledgers
             WHERE conversation_id = ?1 AND run_id != ?2",
            params![snapshot.conversation_id(), snapshot.run_id()],
        )?;
        let affected = transaction.execute(
            "INSERT INTO execution_ledgers
                (run_id, conversation_id, project_id, snapshot_json, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(run_id) DO UPDATE SET
                snapshot_json = excluded.snapshot_json,
                updated_at = excluded.updated_at
             WHERE execution_ledgers.conversation_id = excluded.conversation_id
               AND execution_ledgers.project_id = excluded.project_id",
            params![
                snapshot.run_id(),
                snapshot.conversation_id(),
                snapshot.project_id(),
                serialized,
                updated_at
            ],
        )?;
        if affected != 1 {
            return Err(StorageError::InvalidRecord(
                "execution ledger run scope cannot change",
            ));
        }
        transaction.commit()?;
        Ok(())
    }

    /// Loads and validates one persisted trusted execution ledger snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error when stored state is malformed, invalid, or cannot be
    /// read from the database.
    pub fn execution_ledger(
        &self,
        run_id: &str,
    ) -> Result<Option<ExecutionLedgerSnapshot>, StorageError> {
        if run_id.is_empty() || run_id.len() > 160 || run_id.chars().any(char::is_whitespace) {
            return Err(StorageError::InvalidRecord(
                "execution ledger run ID is invalid",
            ));
        }
        let serialized = self
            .connection
            .query_row(
                "SELECT snapshot_json FROM execution_ledgers WHERE run_id = ?1",
                [run_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        serialized
            .map(|serialized| {
                let snapshot: ExecutionLedgerSnapshot = serde_json::from_str(&serialized)?;
                ExecutionLedger::restore(snapshot.clone()).map_err(|_| {
                    StorageError::InvalidRecord("stored execution ledger is invalid")
                })?;
                Ok(snapshot)
            })
            .transpose()
    }

    /// Loads the validated active ledger for one exact conversation/project scope.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed scope, corrupt stored state or database failure.
    pub fn execution_ledger_for_conversation(
        &self,
        conversation_id: &str,
        project_id: &str,
    ) -> Result<Option<ExecutionLedgerSnapshot>, StorageError> {
        if [conversation_id, project_id].iter().any(|value| {
            value.is_empty() || value.len() > 160 || value.chars().any(char::is_whitespace)
        }) {
            return Err(StorageError::InvalidRecord(
                "execution ledger scope is invalid",
            ));
        }
        let serialized = self
            .connection
            .query_row(
                "SELECT snapshot_json FROM execution_ledgers
                 WHERE conversation_id = ?1 AND project_id = ?2",
                params![conversation_id, project_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        serialized
            .map(|serialized| {
                let snapshot: ExecutionLedgerSnapshot = serde_json::from_str(&serialized)?;
                ExecutionLedger::restore(snapshot.clone()).map_err(|_| {
                    StorageError::InvalidRecord("stored execution ledger is invalid")
                })?;
                Ok(snapshot)
            })
            .transpose()
    }

    /// Deletes one active execution ledger without deleting its conversation.
    ///
    /// # Errors
    ///
    /// Returns an error when the database operation fails.
    pub fn delete_execution_ledger(&self, run_id: &str) -> Result<bool, StorageError> {
        let affected = self
            .connection
            .execute("DELETE FROM execution_ledgers WHERE run_id = ?1", [run_id])?;
        Ok(affected == 1)
    }
}
