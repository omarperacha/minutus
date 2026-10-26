//! Frontend-safe workspace grant metadata.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::WorkspaceGrantId;

/// Permission level attached to a workspace grant.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum WorkspacePermissionLevel {
    /// Read access only.
    ReadOnly,
    /// Read and direct write access within the granted project folder.
    ReadAndWrite,
}

/// Non-authority-bearing metadata for one active workspace grant.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceGrantSummary {
    /// Opaque grant identifier used for later workspace operations.
    pub id: WorkspaceGrantId,
    /// User-visible selected folder name.
    pub display_name: String,
    /// Current capability level.
    pub permission_level: WorkspacePermissionLevel,
    /// RFC 3339 time at which the grant was created.
    pub created_at: String,
}
