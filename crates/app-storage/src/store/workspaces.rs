use super::{
    ApplicationStore, OptionalExtension, ProjectWorkspaceRole, StorageError, WorkspaceGrant,
    WorkspaceGrantId, WorkspacePermissionLevel, params,
};

impl ApplicationStore {
    /// Inserts or refreshes an active workspace grant.
    ///
    /// # Errors
    ///
    /// Returns an error when the grant is not active, bookmark data is empty,
    /// or the database write fails.
    pub fn save_workspace_grant(&self, grant: &WorkspaceGrant) -> Result<(), StorageError> {
        if grant.id.0.is_empty() || grant.bookmark_data.is_empty() || grant.revoked_at.is_some() {
            return Err(StorageError::InvalidRecord(
                "new workspace grants must be active and contain bookmark data",
            ));
        }
        self.connection.execute(
            "INSERT INTO workspace_grants
                (id, display_name, canonical_root, bookmark_data, permission_level, created_at, revoked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)
             ON CONFLICT(id) DO UPDATE SET
                display_name = excluded.display_name,
                canonical_root = excluded.canonical_root,
                bookmark_data = excluded.bookmark_data,
                permission_level = excluded.permission_level,
                created_at = excluded.created_at,
                revoked_at = NULL",
            params![
                grant.id.0,
                grant.display_name,
                grant.canonical_root,
                grant.bookmark_data,
                workspace_permission_name(grant.permission_level),
                grant.created_at,
            ],
        )?;
        Ok(())
    }

    /// Loads an active workspace grant. Revoked grants are never returned as capabilities.
    ///
    /// # Errors
    ///
    /// Returns an error when the database query or record decoding fails.
    pub fn workspace_grant(
        &self,
        grant_id: &WorkspaceGrantId,
    ) -> Result<Option<WorkspaceGrant>, StorageError> {
        self.connection
            .query_row(
                "SELECT display_name, canonical_root, bookmark_data, permission_level, created_at
                 FROM workspace_grants WHERE id = ?1 AND revoked_at IS NULL",
                [&grant_id.0],
                |row| {
                    Ok(WorkspaceGrant {
                        id: grant_id.clone(),
                        display_name: row.get(0)?,
                        canonical_root: row.get(1)?,
                        bookmark_data: row.get(2)?,
                        permission_level: workspace_permission_from_row(row, 3)?,
                        created_at: row.get(4)?,
                        revoked_at: None,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    /// Lists every active workspace grant without returning revoked capabilities.
    ///
    /// # Errors
    ///
    /// Returns an error when persisted grant metadata is malformed or cannot be read.
    pub fn workspace_grants(&self) -> Result<Vec<WorkspaceGrant>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT id, display_name, canonical_root, bookmark_data, permission_level, created_at
             FROM workspace_grants WHERE revoked_at IS NULL
             ORDER BY created_at ASC, id ASC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(WorkspaceGrant {
                id: WorkspaceGrantId(row.get(0)?),
                display_name: row.get(1)?,
                canonical_root: row.get(2)?,
                bookmark_data: row.get(3)?,
                permission_level: workspace_permission_from_row(row, 4)?,
                created_at: row.get(5)?,
                revoked_at: None,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(validate_loaded_workspace_grant)
            .collect()
    }

    /// Finds the active grant for one trusted canonical root.
    ///
    /// # Errors
    ///
    /// Returns an error when the database query or record decoding fails.
    pub fn workspace_grant_for_root(
        &self,
        canonical_root: &str,
    ) -> Result<Option<WorkspaceGrant>, StorageError> {
        let grant = self
            .connection
            .query_row(
                "SELECT id, display_name, bookmark_data, permission_level, created_at
                 FROM workspace_grants
                 WHERE canonical_root = ?1 AND revoked_at IS NULL
                 ORDER BY created_at ASC LIMIT 1",
                [canonical_root],
                |row| {
                    Ok(WorkspaceGrant {
                        id: WorkspaceGrantId(row.get(0)?),
                        display_name: row.get(1)?,
                        canonical_root: canonical_root.to_owned(),
                        bookmark_data: row.get(2)?,
                        permission_level: workspace_permission_from_row(row, 3)?,
                        created_at: row.get(4)?,
                        revoked_at: None,
                    })
                },
            )
            .optional()?;
        grant.map(validate_loaded_workspace_grant).transpose()
    }

    /// Changes the permission level of one active workspace grant.
    ///
    /// # Errors
    ///
    /// Returns an error when the database update or record decoding fails.
    pub fn update_workspace_permission(
        &self,
        grant_id: &WorkspaceGrantId,
        permission_level: WorkspacePermissionLevel,
    ) -> Result<Option<WorkspaceGrant>, StorageError> {
        let affected = self.connection.execute(
            "UPDATE workspace_grants SET permission_level = ?2
             WHERE id = ?1 AND revoked_at IS NULL",
            params![grant_id.0, workspace_permission_name(permission_level)],
        )?;
        if affected == 0 {
            return Ok(None);
        }
        self.workspace_grant(grant_id)
    }

    /// Revokes a workspace grant and erases its persisted bookmark capability.
    ///
    /// # Errors
    ///
    /// Returns an error when the timestamp is empty or the database write fails.
    pub fn revoke_workspace_grant(
        &self,
        grant_id: &WorkspaceGrantId,
        revoked_at: &str,
    ) -> Result<bool, StorageError> {
        if revoked_at.is_empty() {
            return Err(StorageError::InvalidRecord(
                "workspace revocation timestamp must not be empty",
            ));
        }
        let affected = self.connection.execute(
            "UPDATE workspace_grants
             SET bookmark_data = X'', revoked_at = ?2
             WHERE id = ?1 AND revoked_at IS NULL",
            params![grant_id.0, revoked_at],
        )?;
        Ok(affected == 1)
    }
}

pub(super) fn validate_loaded_workspace_grant(
    grant: WorkspaceGrant,
) -> Result<WorkspaceGrant, StorageError> {
    if grant.id.0.is_empty()
        || grant.display_name.is_empty()
        || grant.canonical_root.is_empty()
        || grant.bookmark_data.is_empty()
        || grant.created_at.is_empty()
        || grant.revoked_at.is_some()
    {
        return Err(StorageError::CorruptDatabase);
    }
    Ok(grant)
}

const fn workspace_permission_name(permission: WorkspacePermissionLevel) -> &'static str {
    match permission {
        WorkspacePermissionLevel::ReadOnly => "read_only",
        WorkspacePermissionLevel::ReadAndWrite => "read_and_write",
    }
}

pub(super) const fn project_workspace_role_name(role: ProjectWorkspaceRole) -> &'static str {
    match role {
        ProjectWorkspaceRole::Primary => "primary",
        ProjectWorkspaceRole::Additional => "additional",
    }
}

pub(super) fn project_workspace_role_from_row(
    row: &rusqlite::Row<'_>,
    index: usize,
) -> rusqlite::Result<ProjectWorkspaceRole> {
    match row.get::<_, String>(index)?.as_str() {
        "primary" => Ok(ProjectWorkspaceRole::Primary),
        "additional" => Ok(ProjectWorkspaceRole::Additional),
        _ => Err(rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::new(StorageError::CorruptDatabase),
        )),
    }
}

pub(super) fn workspace_permission_from_row(
    row: &rusqlite::Row<'_>,
    index: usize,
) -> rusqlite::Result<WorkspacePermissionLevel> {
    match row.get::<_, String>(index)?.as_str() {
        "read_only" => Ok(WorkspacePermissionLevel::ReadOnly),
        "read_and_write" => Ok(WorkspacePermissionLevel::ReadAndWrite),
        _ => Err(rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::new(StorageError::CorruptDatabase),
        )),
    }
}
