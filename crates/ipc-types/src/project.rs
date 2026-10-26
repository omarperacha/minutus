//! Frontend-safe project metadata and folder settings.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{ProjectId, WorkspaceGrantId, WorkspacePermissionLevel};

/// Request to create a project independently of any folder grant.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateProjectRequest {
    /// User-visible project name.
    pub name: String,
}

/// Lightweight project metadata used by the project switcher.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectSummary {
    /// Stable project identifier.
    pub id: ProjectId,
    /// User-visible project name.
    pub name: String,
    /// RFC 3339 creation timestamp.
    pub created_at: String,
    /// RFC 3339 timestamp of the most recent project change.
    pub updated_at: String,
}

/// How a granted folder participates in one project.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ProjectWorkspaceRole {
    /// The first folder attached to the project and its default workspace alias.
    Primary,
    /// An optional additional folder available to the same project chats.
    Additional,
}

/// Non-authority-bearing folder setting for one project.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectWorkspaceSummary {
    /// Opaque workspace grant identifier.
    pub grant_id: WorkspaceGrantId,
    /// User-visible selected folder name.
    pub display_name: String,
    /// Folder role within this project.
    pub role: ProjectWorkspaceRole,
    /// Current broker-enforced permission.
    pub permission_level: WorkspacePermissionLevel,
    /// RFC 3339 time at which the folder was attached.
    pub added_at: String,
}

/// Request to change one project folder's permission.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateProjectWorkspacePermissionRequest {
    /// Project that owns the folder setting.
    pub project_id: ProjectId,
    /// Attached workspace grant to update.
    pub grant_id: WorkspaceGrantId,
    /// Replacement permission level.
    pub permission_level: WorkspacePermissionLevel,
}
