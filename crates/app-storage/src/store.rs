//! Typed repository operations over the local application database.

use std::path::Path;

use agent_core::{
    CompactToolResultRecord, ExecutionLedger, ExecutionLedgerSnapshot, RichEvidenceCheckpoint,
    SemanticCompactionCheckpoint,
};
use ipc_types::{
    AssetId, Conversation, ConversationId, ConversationSummary, MAX_CONVERSATION_TITLE_BYTES,
    MAX_PROJECT_NAME_BYTES, Message, MessagePart, MessageRole, ModelSelection, ProjectId,
    ProjectWorkspaceRole, WalletId, WorkspaceGrantId, WorkspacePermissionLevel,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::{
    AssetFileReference, AssetReference, AuditEvent, ModelCompanionMetadata, ModelMetadata, Project,
    ProjectWorkspace, PublicWalletMetadata, SettingKey, SettingValue, StorageError,
    StoredApprovalFinalization, StoredApprovalFinalizationOutcome, StoredEvmTransaction,
    StoredEvmTransactionState, WorkspaceGrant, migration,
};

mod assets;
mod audit;
mod chat_transfers;
mod checkpoints;
mod compact_results;
pub(crate) mod contract_workflows;
mod conversations;
mod dialogue_continuity;
mod evm;
mod evm_dispatch;
pub(crate) mod evm_nonces;
mod execution_ledgers;
pub(crate) mod external_models;
mod model_installs;
mod models;
mod projects;
mod rich_evidence;
mod semantic_compaction;
mod workspaces;

use assets::query_unreferenced_assets;
use models::{integer_u32, integer_u64};
use workspaces::{
    project_workspace_role_from_row, project_workspace_role_name, validate_loaded_workspace_grant,
    workspace_permission_from_row,
};

const ASSET_COLUMNS: &str = "id, storage_key, media_type, content_digest,
    byte_size, width, height, imported_at, original_storage_key,
    original_media_type, original_content_digest, original_byte_size,
    original_width, original_height";

/// Rust-owned handle to Zeta's ordinary local application database.
///
/// This type is intentionally not serializable and is never an IPC value.
pub struct ApplicationStore {
    connection: Connection,
}

impl ApplicationStore {
    /// Opens or creates a Zeta application database and applies pending migrations.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be opened, its identity or schema
    /// is incompatible, integrity validation fails, or a migration fails.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let connection = Connection::open(path)?;
        Self::initialize(connection)
    }

    fn initialize(mut connection: Connection) -> Result<Self, StorageError> {
        migration::initialize(&mut connection)?;
        Ok(Self { connection })
    }

    /// Returns the version of the validated schema.
    ///
    /// # Errors
    ///
    /// Returns an error when the schema version cannot be read safely.
    pub fn schema_version(&self) -> Result<u32, StorageError> {
        let version = self
            .connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?;
        u32::try_from(version).map_err(|_| StorageError::CorruptDatabase)
    }

    /// Inserts or updates one typed, non-secret application setting.
    ///
    /// # Errors
    ///
    /// Returns an error when serialization or the database write fails.
    pub fn set_setting(&self, value: &SettingValue, updated_at: &str) -> Result<(), StorageError> {
        let serialized = serde_json::to_string(value)?;
        self.connection.execute(
            "INSERT INTO settings (key, value_json, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET
                value_json = excluded.value_json,
                updated_at = excluded.updated_at",
            params![value.key().as_str(), serialized, updated_at],
        )?;
        Ok(())
    }

    /// Returns metadata for one managed asset.
    ///
    /// # Errors
    ///
    /// Returns an error when persisted metadata is malformed or cannot be read.
    pub fn setting(&self, key: SettingKey) -> Result<Option<SettingValue>, StorageError> {
        let serialized = self
            .connection
            .query_row(
                "SELECT value_json FROM settings WHERE key = ?1",
                [key.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        serialized
            .map(|value| {
                let decoded: SettingValue = serde_json::from_str(&value)?;
                if decoded.key() != key {
                    return Err(StorageError::InvalidRecord(
                        "setting key does not match its typed value",
                    ));
                }
                Ok(decoded)
            })
            .transpose()
    }

    #[cfg(test)]
    fn open_in_memory() -> Result<Self, StorageError> {
        Self::initialize(Connection::open_in_memory()?)
    }
}

fn to_i64(value: u64) -> Result<i64, StorageError> {
    i64::try_from(value).map_err(|_| StorageError::InvalidRecord("byte size exceeds SQLite range"))
}

#[cfg(test)]
mod tests;
