use super::{ApplicationStore, ModelSelection, StorageError, params};

impl ApplicationStore {
    /// Records provider-owned model bytes independently from provider health.
    ///
    /// # Errors
    ///
    /// Returns an error for an excessive size or database failure.
    pub fn save_provider_model_install(
        &self,
        selection: &ModelSelection,
        installed_bytes: u64,
        installed_at: &str,
    ) -> Result<(), StorageError> {
        let installed_bytes = i64::try_from(installed_bytes)
            .map_err(|_| StorageError::InvalidRecord("model size exceeds SQLite range"))?;
        self.connection.execute(
            "INSERT INTO provider_model_installs
                (model_id, model_version, installed_bytes, installed_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(model_id, model_version) DO UPDATE SET
                installed_bytes = excluded.installed_bytes,
                installed_at = excluded.installed_at",
            params![
                selection.model_id,
                selection.model_version,
                installed_bytes,
                installed_at
            ],
        )?;
        Ok(())
    }

    /// Returns whether provider-owned bytes were previously verified and registered.
    ///
    /// # Errors
    ///
    /// Returns an error when the database query fails.
    pub fn provider_model_installed(
        &self,
        selection: &ModelSelection,
    ) -> Result<bool, StorageError> {
        let count: i64 = self.connection.query_row(
            "SELECT count(*) FROM provider_model_installs WHERE model_id = ?1 AND model_version = ?2",
            params![selection.model_id, selection.model_version],
            |row| row.get(0),
        )?;
        Ok(count == 1)
    }

    /// Removes a stale provider install receipt after an available provider confirms absence.
    ///
    /// # Errors
    ///
    /// Returns an error when the database update fails.
    pub fn delete_provider_model_install(
        &self,
        selection: &ModelSelection,
    ) -> Result<(), StorageError> {
        self.connection.execute(
            "DELETE FROM provider_model_installs WHERE model_id = ?1 AND model_version = ?2",
            params![selection.model_id, selection.model_version],
        )?;
        Ok(())
    }
}
