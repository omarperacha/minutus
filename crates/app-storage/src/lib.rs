//! Rust-owned local application persistence for Zeta.
//!
//! This crate owns the ordinary application database. It deliberately has no
//! frontend integration and no wallet-vault API: recovery phrases, private
//! keys and signing material belong in the separate authenticated vault owned
//! by `wallet-core`.

mod error;
mod migration;
mod records;
mod store;

pub use error::StorageError;
pub use migration::SCHEMA_VERSION;
pub use records::{
    AssetFileReference, AssetReference, AuditEvent, AuditEventKind, AuditFilesystemTool,
    AuditOutcome, AuditPermissionDecision, ModelCompanionMetadata, ModelMetadata, Project,
    ProjectWorkspace, PublicWalletMetadata, SettingKey, SettingValue, StoredApprovalFinalization,
    StoredApprovalFinalizationOutcome, StoredEvmDispatchPhase, StoredEvmTransaction,
    StoredEvmTransactionState, Theme, WorkspaceGrant,
};
pub use store::ApplicationStore;
pub use store::external_models::{ExternalModelFile, ExternalModelRegistration};
