use super::{
    ApplicationStore, MAX_PROJECT_NAME_BYTES, ModelSelection, OptionalExtension, Project,
    ProjectId, ProjectWorkspace, ProjectWorkspaceRole, StorageError, TransactionBehavior,
    WorkspaceGrant, WorkspaceGrantId, params, project_workspace_role_from_row,
    project_workspace_role_name, validate_loaded_workspace_grant, workspace_permission_from_row,
};

impl ApplicationStore {
    /// Inserts one project.
    ///
    /// # Errors
    ///
    /// Returns an error when required metadata is empty or persistence fails.
    pub fn save_project(&self, project: &Project) -> Result<(), StorageError> {
        if project.id.0.is_empty()
            || project.name.is_empty()
            || project.name.trim() != project.name
            || project.name.len() > MAX_PROJECT_NAME_BYTES
            || project.created_at.is_empty()
            || project.updated_at.is_empty()
        {
            return Err(StorageError::InvalidRecord("project metadata is invalid"));
        }
        self.connection.execute(
            "INSERT INTO projects (id, name, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                project.id.0,
                project.name,
                project.created_at,
                project.updated_at
            ],
        )?;
        Ok(())
    }

    /// Lists projects newest-first.
    ///
    /// # Errors
    ///
    /// Returns an error when persisted project metadata cannot be decoded.
    pub fn projects(&self) -> Result<Vec<Project>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT id, name, created_at, updated_at FROM projects
             ORDER BY updated_at DESC, id ASC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(Project {
                id: ProjectId(row.get(0)?),
                name: row.get(1)?,
                created_at: row.get(2)?,
                updated_at: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Loads one project by opaque identifier.
    ///
    /// # Errors
    ///
    /// Returns an error when persistence fails.
    pub fn project(&self, project_id: &ProjectId) -> Result<Option<Project>, StorageError> {
        self.connection
            .query_row(
                "SELECT name, created_at, updated_at FROM projects WHERE id = ?1",
                [&project_id.0],
                |row| {
                    Ok(Project {
                        id: project_id.clone(),
                        name: row.get(0)?,
                        created_at: row.get(1)?,
                        updated_at: row.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    /// Loads the optional exact model release selected for one project.
    ///
    /// # Errors
    ///
    /// Returns an error for incomplete stored identity or database failure.
    pub fn project_model(
        &self,
        project_id: &ProjectId,
    ) -> Result<Option<ModelSelection>, StorageError> {
        self.connection
            .query_row(
                "SELECT model_id, model_version FROM projects WHERE id = ?1",
                [&project_id.0],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                    ))
                },
            )
            .optional()?
            .map(|(id, version)| match (id, version) {
                (Some(model_id), Some(model_version)) => Ok(Some(ModelSelection {
                    model_id,
                    model_version,
                })),
                (None, None) => Ok(None),
                _ => Err(StorageError::InvalidRecord(
                    "project model identity is incomplete",
                )),
            })
            .transpose()
            .map(Option::flatten)
    }

    /// Replaces or clears one project's exact model preference.
    ///
    /// # Errors
    ///
    /// Returns an error when the project is absent, identity is invalid, or
    /// persistence fails.
    pub fn set_project_model(
        &mut self,
        project_id: &ProjectId,
        selection: Option<&ModelSelection>,
        updated_at: &str,
    ) -> Result<bool, StorageError> {
        if selection.is_some_and(|selection| {
            selection.model_id.is_empty() || selection.model_version.is_empty()
        }) {
            return Err(StorageError::InvalidRecord(
                "model identity must not be empty",
            ));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (model_id, model_version) = selection.map_or((None, None), |selection| {
            (
                Some(selection.model_id.as_str()),
                Some(selection.model_version.as_str()),
            )
        });
        let changed = transaction.execute(
            "UPDATE projects SET model_id = ?1, model_version = ?2, updated_at = ?3
             WHERE id = ?4 AND (model_id IS NOT ?1 OR model_version IS NOT ?2)",
            params![model_id, model_version, updated_at, project_id.0],
        )?;
        transaction.commit()?;
        Ok(changed != 0)
    }

    /// Attaches an active workspace grant to a project.
    ///
    /// # Errors
    ///
    /// Returns an error when either owner is absent or persistence fails.
    pub fn attach_project_workspace(
        &self,
        project_id: &ProjectId,
        grant_id: &WorkspaceGrantId,
        role: ProjectWorkspaceRole,
        added_at: &str,
    ) -> Result<(), StorageError> {
        self.connection.execute(
            "INSERT INTO project_workspaces
                (project_id, workspace_grant_id, role, added_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(project_id, workspace_grant_id) DO UPDATE SET
                added_at = excluded.added_at",
            params![
                project_id.0,
                grant_id.0,
                project_workspace_role_name(role),
                added_at
            ],
        )?;
        self.connection.execute(
            "UPDATE projects SET updated_at = ?1 WHERE id = ?2",
            params![added_at, project_id.0],
        )?;
        Ok(())
    }

    /// Lists active granted folders belonging to one project.
    ///
    /// # Errors
    ///
    /// Returns an error when persisted metadata cannot be decoded safely.
    pub fn project_workspaces(
        &self,
        project_id: &ProjectId,
    ) -> Result<Vec<ProjectWorkspace>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT g.id, g.display_name, g.canonical_root, g.bookmark_data,
                    g.permission_level, g.created_at, pw.role, pw.added_at
             FROM project_workspaces pw
             JOIN workspace_grants g ON g.id = pw.workspace_grant_id
             WHERE pw.project_id = ?1 AND g.revoked_at IS NULL
             ORDER BY CASE pw.role WHEN 'primary' THEN 0 ELSE 1 END, pw.added_at, g.id",
        )?;
        let rows = statement.query_map([&project_id.0], |row| {
            Ok(ProjectWorkspace {
                project_id: project_id.clone(),
                grant: WorkspaceGrant {
                    id: WorkspaceGrantId(row.get(0)?),
                    display_name: row.get(1)?,
                    canonical_root: row.get(2)?,
                    bookmark_data: row.get(3)?,
                    permission_level: workspace_permission_from_row(row, 4)?,
                    created_at: row.get(5)?,
                    revoked_at: None,
                },
                role: project_workspace_role_from_row(row, 6)?,
                added_at: row.get(7)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|workspace| {
                validate_loaded_workspace_grant(workspace.grant.clone())?;
                Ok(workspace)
            })
            .collect()
    }

    /// Returns whether a project currently owns an active workspace grant.
    ///
    /// # Errors
    ///
    /// Returns an error when persistence fails.
    pub fn project_has_workspace(
        &self,
        project_id: &ProjectId,
        grant_id: &WorkspaceGrantId,
    ) -> Result<bool, StorageError> {
        let count: i64 = self.connection.query_row(
            "SELECT count(*) FROM project_workspaces pw
             JOIN workspace_grants g ON g.id = pw.workspace_grant_id
             WHERE pw.project_id = ?1 AND pw.workspace_grant_id = ?2
               AND g.revoked_at IS NULL",
            params![project_id.0, grant_id.0],
            |row| row.get(0),
        )?;
        Ok(count == 1)
    }

    /// Detaches one folder and promotes the oldest additional folder when needed.
    ///
    /// # Errors
    ///
    /// Returns an error when the transaction fails.
    pub fn detach_project_workspace(
        &mut self,
        project_id: &ProjectId,
        grant_id: &WorkspaceGrantId,
        updated_at: &str,
    ) -> Result<bool, StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let role = transaction
            .query_row(
                "SELECT role FROM project_workspaces
                 WHERE project_id = ?1 AND workspace_grant_id = ?2",
                params![project_id.0, grant_id.0],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(role) = role else {
            return Ok(false);
        };
        transaction.execute(
            "DELETE FROM project_workspaces
             WHERE project_id = ?1 AND workspace_grant_id = ?2",
            params![project_id.0, grant_id.0],
        )?;
        if role == "primary" {
            transaction.execute(
                "UPDATE project_workspaces SET role = 'primary'
                 WHERE project_id = ?1 AND workspace_grant_id = (
                    SELECT workspace_grant_id FROM project_workspaces
                    WHERE project_id = ?1 ORDER BY added_at, workspace_grant_id LIMIT 1
                 )",
                [&project_id.0],
            )?;
        }
        transaction.execute(
            "UPDATE projects SET updated_at = ?1 WHERE id = ?2",
            params![updated_at, project_id.0],
        )?;
        transaction.commit()?;
        Ok(true)
    }

    /// Counts project associations for one active grant.
    ///
    /// # Errors
    ///
    /// Returns an error when persistence fails.
    pub fn workspace_project_count(
        &self,
        grant_id: &WorkspaceGrantId,
    ) -> Result<u64, StorageError> {
        let count: i64 = self.connection.query_row(
            "SELECT count(*) FROM project_workspaces WHERE workspace_grant_id = ?1",
            [&grant_id.0],
            |row| row.get(0),
        )?;
        u64::try_from(count).map_err(|_| StorageError::CorruptDatabase)
    }
}
