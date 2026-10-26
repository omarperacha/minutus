use super::{
    ApplicationStore, AssetReference, CompactToolResultRecord, RichEvidenceCheckpoint,
    SemanticCompactionCheckpoint, StorageError, compact_results, rich_evidence,
    semantic_compaction,
};

impl ApplicationStore {
    /// Persists complete retained tool content outside conversation messages.
    ///
    /// Conversation/project ownership is enforced by foreign keys and a scope
    /// trigger. Managed assets referenced by the record remain live until the
    /// record expires or its owning conversation is deleted.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid record, missing scope/asset, duplicate
    /// identity, serialization failure or database failure.
    pub fn save_compact_tool_result(
        &mut self,
        record: &CompactToolResultRecord,
    ) -> Result<(), StorageError> {
        compact_results::save(self, record)
    }

    /// Loads one retained result only through its exact conversation/project scope.
    ///
    /// The returned host-only record must still pass `rehydrate` with current
    /// capability and source-revision evidence before its payload is released.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed IDs, corrupt metadata or database failure.
    pub fn compact_tool_result(
        &self,
        id: &str,
        conversation_id: &str,
        project_id: &str,
    ) -> Result<Option<CompactToolResultRecord>, StorageError> {
        compact_results::load(self, id, conversation_id, project_id)
    }

    /// Deletes expired retained results and reaps assets they leave unreferenced.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid timestamp, corrupt asset metadata or a
    /// database failure.
    pub fn reap_expired_compact_results(
        &mut self,
        now: &str,
    ) -> Result<(u64, Vec<AssetReference>), StorageError> {
        compact_results::reap_expired(self, now)
    }

    /// Inserts or replaces one validated semantic compaction checkpoint.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid scope, corrupt metadata, serialization or
    /// database failure.
    pub fn save_semantic_compaction_checkpoint(
        &mut self,
        checkpoint: &SemanticCompactionCheckpoint,
    ) -> Result<(), StorageError> {
        semantic_compaction::save(self, checkpoint)
    }

    /// Loads the semantic checkpoint for one exact conversation/project scope.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed scope, corrupt stored metadata or database
    /// failure.
    pub fn semantic_compaction_checkpoint(
        &self,
        conversation_id: &str,
        project_id: &str,
    ) -> Result<Option<SemanticCompactionCheckpoint>, StorageError> {
        semantic_compaction::load(self, conversation_id, project_id)
    }

    /// Removes a stale semantic checkpoint from one exact scope.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed scope or database failure.
    pub fn delete_semantic_compaction_checkpoint(
        &mut self,
        conversation_id: &str,
        project_id: &str,
    ) -> Result<bool, StorageError> {
        semantic_compaction::delete(self, conversation_id, project_id)
    }

    /// Inserts or replaces one validated rich-evidence checkpoint.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid scope, corrupt metadata, serialization or
    /// database failure.
    pub fn save_rich_evidence_checkpoint(
        &mut self,
        checkpoint: &RichEvidenceCheckpoint,
    ) -> Result<(), StorageError> {
        rich_evidence::save(self, checkpoint)
    }

    /// Loads the rich-evidence checkpoint for one exact conversation/project scope.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed scope, corrupt stored metadata or database
    /// failure.
    pub fn rich_evidence_checkpoint(
        &self,
        conversation_id: &str,
        project_id: &str,
    ) -> Result<Option<RichEvidenceCheckpoint>, StorageError> {
        rich_evidence::load(self, conversation_id, project_id)
    }

    /// Removes a stale rich-evidence checkpoint from one exact scope.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed scope or database failure.
    pub fn delete_rich_evidence_checkpoint(
        &mut self,
        conversation_id: &str,
        project_id: &str,
    ) -> Result<bool, StorageError> {
        rich_evidence::delete(self, conversation_id, project_id)
    }
}
