//! Managed assets and cleanup coupled to persisted conversation references.

use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{AppCore, AppCoreError, AssetId, AssetReference, Conversation, ConversationId};

impl AppCore {
    /// Imports one bounded image from bytes into private managed storage.
    ///
    /// No source path or URL is accepted or persisted. File publication occurs
    /// before the metadata commit; a failed commit rolls the files back.
    ///
    /// # Errors
    ///
    /// Returns a sanitized error when validation, normalization or persistence fails.
    pub fn import_image(
        &self,
        bytes: &[u8],
        imported_at: &str,
    ) -> Result<AssetReference, AppCoreError> {
        let _operation = self
            .asset_operations
            .lock()
            .map_err(|_| AppCoreError::AssetOperationsUnavailable)?;
        let imported = self.asset_files.import_image(bytes, imported_at)?;
        self.with_storage(|storage| storage.save_asset(imported.metadata()))?;
        Ok(imported.commit())
    }

    /// Removes one pending image that has not been referenced by a saved message.
    ///
    /// Referenced and unknown assets are left untouched, making this safe for a
    /// stale frontend removal request after a message has already been accepted.
    ///
    /// # Errors
    ///
    /// Returns an error when metadata or private-file cleanup fails.
    pub fn discard_unreferenced_image(&self, asset_id: &AssetId) -> Result<bool, AppCoreError> {
        let _operation = self
            .asset_operations
            .lock()
            .map_err(|_| AppCoreError::AssetOperationsUnavailable)?;
        let asset = self.with_storage(|storage| storage.delete_unreferenced_asset(asset_id))?;
        if let Some(asset) = asset {
            self.asset_files.remove_assets(&[asset])?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Saves a conversation and removes assets made unreferenced by replacement.
    ///
    /// # Errors
    ///
    /// Returns an error when the database transaction or subsequent file cleanup fails.
    pub fn save_conversation(&self, conversation: &Conversation) -> Result<(), AppCoreError> {
        let _operation = self
            .asset_operations
            .lock()
            .map_err(|_| AppCoreError::AssetOperationsUnavailable)?;
        let reaped = self.with_storage(|storage| {
            storage.save_conversation(conversation)?;
            storage.reap_unreferenced_assets()
        })?;
        self.asset_files.remove_assets(&reaped)?;
        Ok(())
    }

    /// Deletes a conversation and any managed assets no other message references.
    ///
    /// # Errors
    ///
    /// Returns an error when the database transaction or file cleanup fails.
    /// Database deletion remains committed if later file removal fails; startup
    /// reconciliation safely retries the remaining orphan cleanup.
    pub fn delete_conversation(
        &self,
        conversation_id: &ConversationId,
    ) -> Result<bool, AppCoreError> {
        let _operation = self
            .asset_operations
            .lock()
            .map_err(|_| AppCoreError::AssetOperationsUnavailable)?;
        let (deleted, reaped) = self
            .with_storage(|storage| storage.delete_conversation_and_reap_assets(conversation_id))?;
        self.asset_files.remove_assets(&reaped)?;
        Ok(deleted)
    }

    /// Reconciles database references and managed files after an interrupted operation.
    ///
    /// # Errors
    ///
    /// Returns an error when metadata or private file cleanup fails.
    pub fn cleanup_assets(&self) -> Result<(), AppCoreError> {
        let _operation = self
            .asset_operations
            .lock()
            .map_err(|_| AppCoreError::AssetOperationsUnavailable)?;
        let now = current_retention_timestamp();
        let (_, reaped) =
            self.with_storage(|storage| storage.reap_expired_compact_results(&now))?;
        self.asset_files.remove_assets(&reaped)?;
        let live = self.with_storage(|storage| storage.assets())?;
        self.asset_files.reconcile(&live)?;
        Ok(())
    }
}

fn current_retention_timestamp() -> String {
    OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH)
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}
