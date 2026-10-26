use super::{
    ApplicationStore, ModelCompanionMetadata, ModelMetadata, OptionalExtension, StorageError,
    TransactionBehavior, params, to_i64,
};

impl ApplicationStore {
    /// Inserts or updates metadata for an installed local model.
    ///
    /// # Errors
    ///
    /// Returns an error when the byte size exceeds the database range or the
    /// database write fails.
    pub fn save_model(&self, model: &ModelMetadata) -> Result<(), StorageError> {
        self.connection.execute(
            "INSERT INTO models
                (id, display_name, format, local_path, content_digest, byte_size, imported_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET
                display_name = excluded.display_name,
                format = excluded.format,
                local_path = excluded.local_path,
                content_digest = excluded.content_digest,
                byte_size = excluded.byte_size,
                imported_at = excluded.imported_at",
            params![
                model.id,
                model.display_name,
                model.format,
                model.local_path,
                model.content_digest,
                to_i64(model.byte_size)?,
                model.imported_at
            ],
        )?;
        Ok(())
    }

    /// Atomically publishes one complete model and its exact companion set.
    ///
    /// Existing companion metadata is replaced as part of the same transaction,
    /// so readers observe either the previous bundle or the complete new bundle.
    ///
    /// # Errors
    ///
    /// Returns an error when companion ownership is inconsistent, sizes exceed
    /// the database range, or the database transaction cannot commit.
    pub fn save_model_bundle(
        &mut self,
        model: &ModelMetadata,
        companions: &[ModelCompanionMetadata],
    ) -> Result<(), StorageError> {
        let mut artifact_ids = std::collections::HashSet::with_capacity(companions.len());
        for companion in companions {
            if companion.model_id != model.id {
                return Err(StorageError::InvalidRecord(
                    "model bundle companion owner does not match primary model",
                ));
            }
            if !artifact_ids.insert(companion.artifact_id.as_str()) {
                return Err(StorageError::InvalidRecord(
                    "model bundle contains a duplicate companion",
                ));
            }
            to_i64(companion.byte_size)?;
            to_i64(companion.memory_requirement_bytes)?;
        }
        let model_size = to_i64(model.byte_size)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT INTO models
                (id, display_name, format, local_path, content_digest, byte_size, imported_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET
                display_name = excluded.display_name,
                format = excluded.format,
                local_path = excluded.local_path,
                content_digest = excluded.content_digest,
                byte_size = excluded.byte_size,
                imported_at = excluded.imported_at",
            params![
                model.id,
                model.display_name,
                model.format,
                model.local_path,
                model.content_digest,
                model_size,
                model.imported_at
            ],
        )?;
        transaction.execute(
            "DELETE FROM model_companions WHERE model_id = ?1",
            [&model.id],
        )?;
        for companion in companions {
            transaction.execute(
                "INSERT INTO model_companions
                    (model_id, artifact_id, kind, required, local_path, content_digest,
                     byte_size, compatibility_version, memory_requirement_bytes, installed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    companion.model_id,
                    companion.artifact_id,
                    companion.kind,
                    companion.required,
                    companion.local_path,
                    companion.content_digest,
                    to_i64(companion.byte_size)?,
                    companion.compatibility_version,
                    to_i64(companion.memory_requirement_bytes)?,
                    companion.installed_at,
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// Returns persisted metadata for one installed local model.
    ///
    /// # Errors
    ///
    /// Returns an error when stored metadata is malformed or cannot be read.
    pub fn model(&self, id: &str) -> Result<Option<ModelMetadata>, StorageError> {
        self.connection
            .query_row(
                "SELECT id, display_name, format, local_path, content_digest,
                        byte_size, imported_at
                 FROM models WHERE id = ?1",
                [id],
                |row| {
                    Ok(ModelMetadata {
                        id: row.get(0)?,
                        display_name: row.get(1)?,
                        format: row.get(2)?,
                        local_path: row.get(3)?,
                        content_digest: row.get(4)?,
                        byte_size: integer_u64(row, 5)?,
                        imported_at: row.get(6)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    /// Inserts or updates metadata for one independently verified model companion.
    ///
    /// # Errors
    ///
    /// Returns an error when sizes exceed the database range, the owning model
    /// does not exist or the database write fails.
    pub fn save_model_companion(
        &self,
        companion: &ModelCompanionMetadata,
    ) -> Result<(), StorageError> {
        self.connection.execute(
            "INSERT INTO model_companions
                (model_id, artifact_id, kind, required, local_path, content_digest,
                 byte_size, compatibility_version, memory_requirement_bytes, installed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(model_id, artifact_id) DO UPDATE SET
                kind = excluded.kind,
                required = excluded.required,
                local_path = excluded.local_path,
                content_digest = excluded.content_digest,
                byte_size = excluded.byte_size,
                compatibility_version = excluded.compatibility_version,
                memory_requirement_bytes = excluded.memory_requirement_bytes,
                installed_at = excluded.installed_at",
            params![
                companion.model_id,
                companion.artifact_id,
                companion.kind,
                companion.required,
                companion.local_path,
                companion.content_digest,
                to_i64(companion.byte_size)?,
                companion.compatibility_version,
                to_i64(companion.memory_requirement_bytes)?,
                companion.installed_at,
            ],
        )?;
        Ok(())
    }

    /// Returns persisted metadata for one installed companion artifact.
    ///
    /// # Errors
    ///
    /// Returns an error when stored metadata is malformed or cannot be read.
    pub fn model_companion(
        &self,
        model_id: &str,
        artifact_id: &str,
    ) -> Result<Option<ModelCompanionMetadata>, StorageError> {
        self.connection
            .query_row(
                "SELECT model_id, artifact_id, kind, required, local_path,
                        content_digest, byte_size, compatibility_version,
                        memory_requirement_bytes, installed_at
                 FROM model_companions WHERE model_id = ?1 AND artifact_id = ?2",
                params![model_id, artifact_id],
                model_companion_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    /// Returns all installed companions for one primary model.
    ///
    /// # Errors
    ///
    /// Returns an error when stored metadata is malformed or cannot be read.
    pub fn model_companions(
        &self,
        model_id: &str,
    ) -> Result<Vec<ModelCompanionMetadata>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT model_id, artifact_id, kind, required, local_path,
                    content_digest, byte_size, compatibility_version,
                    memory_requirement_bytes, installed_at
             FROM model_companions WHERE model_id = ?1 ORDER BY artifact_id",
        )?;
        statement
            .query_map([model_id], model_companion_from_row)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }
}

fn model_companion_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ModelCompanionMetadata> {
    Ok(ModelCompanionMetadata {
        model_id: row.get(0)?,
        artifact_id: row.get(1)?,
        kind: row.get(2)?,
        required: row.get(3)?,
        local_path: row.get(4)?,
        content_digest: row.get(5)?,
        byte_size: integer_u64(row, 6)?,
        compatibility_version: row.get(7)?,
        memory_requirement_bytes: integer_u64(row, 8)?,
        installed_at: row.get(9)?,
    })
}

pub(super) fn integer_u64(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value = row.get::<_, i64>(index)?;
    u64::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}

pub(super) fn integer_u32(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u32> {
    let value = row.get::<_, i64>(index)?;
    u32::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}
