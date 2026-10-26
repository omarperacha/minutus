use super::{
    ASSET_COLUMNS, ApplicationStore, AssetFileReference, AssetId, AssetReference, Connection,
    ConversationId, OptionalExtension, StorageError, TransactionBehavior, integer_u32, integer_u64,
    params, to_i64,
};

impl ApplicationStore {
    /// Returns metadata for one managed asset.
    ///
    /// # Errors
    ///
    /// Returns an error when persisted metadata is malformed or cannot be read.
    pub fn asset(&self, asset_id: &AssetId) -> Result<Option<AssetReference>, StorageError> {
        let query = format!("SELECT {ASSET_COLUMNS} FROM assets WHERE id = ?1");
        let asset = self
            .connection
            .query_row(&query, [&asset_id.0], decode_asset_row)
            .optional()?;
        asset.map(validate_loaded_asset).transpose()
    }

    /// Lists all committed managed-asset metadata for reconciliation.
    ///
    /// # Errors
    ///
    /// Returns an error when persisted metadata is malformed or cannot be read.
    pub fn assets(&self) -> Result<Vec<AssetReference>, StorageError> {
        let query = format!("SELECT {ASSET_COLUMNS} FROM assets ORDER BY id");
        let mut statement = self.connection.prepare(&query)?;
        let rows = statement.query_map([], decode_asset_row)?;
        rows.collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(validate_loaded_asset)
            .collect()
    }

    /// Inserts one immutable validated stable asset reference.
    ///
    /// # Errors
    ///
    /// Returns an error when the storage key or dimensions are invalid, a size
    /// exceeds the database range, or the database write fails.
    pub fn save_asset(&self, asset: &AssetReference) -> Result<(), StorageError> {
        validate_asset(asset)?;
        self.connection.execute(
            "INSERT INTO assets
                (id, storage_key, media_type, content_digest, byte_size,
                 width, height, imported_at, original_storage_key,
                 original_media_type, original_content_digest,
                 original_byte_size, original_width, original_height)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                asset.id.0,
                asset.derivative.storage_key,
                asset.derivative.media_type,
                asset.derivative.content_digest,
                to_i64(asset.derivative.byte_size)?,
                i64::from(asset.derivative.width),
                i64::from(asset.derivative.height),
                asset.imported_at,
                asset.original.storage_key,
                asset.original.media_type,
                asset.original.content_digest,
                to_i64(asset.original.byte_size)?,
                i64::from(asset.original.width),
                i64::from(asset.original.height)
            ],
        )?;
        Ok(())
    }

    /// Returns stable IDs for assets not referenced by any conversation message.
    ///
    /// # Errors
    ///
    /// Returns an error when the database query fails.
    pub fn unreferenced_asset_ids(&self) -> Result<Vec<AssetId>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT assets.id FROM assets
             WHERE NOT EXISTS (
                SELECT 1 FROM message_assets WHERE message_assets.asset_id = assets.id
             ) AND NOT EXISTS (
                SELECT 1 FROM compact_result_assets
                WHERE compact_result_assets.asset_id = assets.id
             ) ORDER BY assets.id",
        )?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0).map(AssetId))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Deletes one asset only when no persisted message references it.
    ///
    /// The returned metadata identifies private files that may be removed after
    /// the transaction commits. A referenced or unknown asset is left intact.
    ///
    /// # Errors
    ///
    /// Returns an error when the database transaction fails or stored metadata
    /// cannot be decoded safely.
    pub fn delete_unreferenced_asset(
        &mut self,
        asset_id: &AssetId,
    ) -> Result<Option<AssetReference>, StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let query = format!(
            "SELECT {ASSET_COLUMNS} FROM assets
             WHERE id = ?1 AND NOT EXISTS (
                SELECT 1 FROM message_assets WHERE message_assets.asset_id = assets.id
             ) AND NOT EXISTS (
                SELECT 1 FROM compact_result_assets
                WHERE compact_result_assets.asset_id = assets.id
             )"
        );
        let asset = transaction
            .query_row(&query, [&asset_id.0], decode_asset_row)
            .optional()?
            .map(validate_loaded_asset)
            .transpose()?;
        if asset.is_some() {
            transaction.execute(
                "DELETE FROM assets
                 WHERE id = ?1 AND NOT EXISTS (
                    SELECT 1 FROM message_assets WHERE message_assets.asset_id = assets.id
                 ) AND NOT EXISTS (
                    SELECT 1 FROM compact_result_assets
                    WHERE compact_result_assets.asset_id = assets.id
                 )",
                [&asset_id.0],
            )?;
        }
        transaction.commit()?;
        Ok(asset)
    }

    /// Atomically deletes a conversation and metadata for assets it leaves unreferenced.
    ///
    /// Returned metadata identifies files that the asset store can delete after commit.
    /// A crash before file deletion leaves harmless orphans for startup reconciliation.
    ///
    /// # Errors
    ///
    /// Returns an error when the database transaction fails.
    pub fn delete_conversation_and_reap_assets(
        &mut self,
        conversation_id: &ConversationId,
    ) -> Result<(bool, Vec<AssetReference>), StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let affected = transaction.execute(
            "DELETE FROM conversations WHERE id = ?1",
            [&conversation_id.0],
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
        Ok((affected == 1, assets))
    }

    /// Reaps metadata for any unreferenced assets during startup recovery.
    ///
    /// # Errors
    ///
    /// Returns an error when the database transaction fails.
    pub fn reap_unreferenced_assets(&mut self) -> Result<Vec<AssetReference>, StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
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
        Ok(assets)
    }
}

pub(super) fn query_unreferenced_assets(
    connection: &Connection,
) -> Result<Vec<AssetReference>, StorageError> {
    let query = format!(
        "SELECT {ASSET_COLUMNS} FROM assets
         WHERE NOT EXISTS (
            SELECT 1 FROM message_assets WHERE message_assets.asset_id = assets.id
         ) AND NOT EXISTS (
            SELECT 1 FROM compact_result_assets
            WHERE compact_result_assets.asset_id = assets.id
         ) ORDER BY id"
    );
    let mut statement = connection.prepare(&query)?;
    let rows = statement.query_map([], decode_asset_row)?;
    rows.collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(validate_loaded_asset)
        .collect()
}

fn validate_loaded_asset(asset: AssetReference) -> Result<AssetReference, StorageError> {
    validate_asset(&asset)?;
    Ok(asset)
}

fn decode_asset_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AssetReference> {
    Ok(AssetReference {
        id: AssetId(row.get(0)?),
        derivative: AssetFileReference {
            storage_key: row.get(1)?,
            media_type: row.get(2)?,
            content_digest: row.get(3)?,
            byte_size: integer_u64(row, 4)?,
            width: integer_u32(row, 5)?,
            height: integer_u32(row, 6)?,
        },
        imported_at: row.get(7)?,
        original: AssetFileReference {
            storage_key: row.get(8)?,
            media_type: row.get(9)?,
            content_digest: row.get(10)?,
            byte_size: integer_u64(row, 11)?,
            width: integer_u32(row, 12)?,
            height: integer_u32(row, 13)?,
        },
    })
}

fn validate_asset(asset: &AssetReference) -> Result<(), StorageError> {
    if asset.id.0.is_empty() || asset.imported_at.is_empty() {
        return Err(StorageError::InvalidRecord(
            "asset identifiers and import timestamps must not be empty",
        ));
    }
    if asset.original.storage_key == asset.derivative.storage_key {
        return Err(StorageError::InvalidRecord(
            "original and derivative storage keys must be distinct",
        ));
    }
    validate_asset_file(&asset.original, "originals/")?;
    validate_asset_file(&asset.derivative, "inference/")?;
    if asset.derivative.media_type != "image/png" {
        return Err(StorageError::InvalidRecord(
            "inference derivatives must be normalized PNG images",
        ));
    }
    Ok(())
}

fn validate_asset_file(file: &AssetFileReference, prefix: &str) -> Result<(), StorageError> {
    let safe_key = file.storage_key.starts_with(prefix)
        && !file.storage_key.starts_with('/')
        && !file
            .storage_key
            .split('/')
            .any(|component| component == "..")
        && file.storage_key.split('/').count() == 2;
    let valid_digest = file.content_digest.len() == 64
        && file
            .content_digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if !safe_key
        || file.byte_size == 0
        || file.width == 0
        || file.height == 0
        || !valid_digest
        || !matches!(
            file.media_type.as_str(),
            "image/jpeg" | "image/png" | "image/webp"
        )
    {
        return Err(StorageError::InvalidRecord(
            "asset file metadata is invalid",
        ));
    }
    Ok(())
}
