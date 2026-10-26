use super::{
    AppCore, AppCoreError, AuditEventKind, MAX_PROJECT_NAME_BYTES, Path, Project, ProjectId,
    ProjectSummary, ProjectWorkspaceRole, ProjectWorkspaceSummary, StorageError, WorkspaceGrant,
    WorkspaceGrantId, WorkspaceGrantSummary, WorkspaceOperation, WorkspacePermissionLevel, fs,
    project_summary, workspace_summary,
};

mod filesystem;
mod operation_gate;

#[cfg(test)]
mod tests;

pub(crate) use operation_gate::WorkspaceOperationGate;

impl AppCore {
    pub(crate) fn open_workspace(
        &self,
        root: impl AsRef<Path>,
    ) -> Result<filesystem_tools::WorkspaceDirectory, filesystem_tools::SafePathError> {
        filesystem_tools::WorkspaceDirectory::open_excluding(root, &self.private_data)
    }

    /// Creates or refreshes a read-only grant for one user-selected workspace.
    ///
    /// The canonical root and opaque bookmark remain in Rust-owned persistence;
    /// only non-authority-bearing summary metadata is returned to the frontend.
    ///
    /// # Errors
    ///
    /// Returns an error when the selection is not a readable directory, cannot
    /// be represented safely, has no bookmark capability, or cannot be stored.
    pub fn grant_workspace(
        &self,
        selected_root: impl AsRef<Path>,
        bookmark_data: Vec<u8>,
        created_at: &str,
    ) -> Result<WorkspaceGrantSummary, AppCoreError> {
        self.grant_workspace_internal(selected_root, bookmark_data, created_at, true)
    }

    fn grant_workspace_internal(
        &self,
        selected_root: impl AsRef<Path>,
        bookmark_data: Vec<u8>,
        created_at: &str,
        reuse_existing: bool,
    ) -> Result<WorkspaceGrantSummary, AppCoreError> {
        let _workspace_guard = self.write_workspace_operations()?;
        if bookmark_data.is_empty() || created_at.is_empty() {
            return Err(AppCoreError::InvalidWorkspaceSelection);
        }
        let canonical_root = fs::canonicalize(selected_root.as_ref())?;
        if !fs::metadata(&canonical_root)?.is_dir() {
            return Err(AppCoreError::InvalidWorkspaceSelection);
        }
        self.open_workspace(&canonical_root)
            .map_err(super::FilesystemToolError::from)?;
        let canonical_root = canonical_root
            .to_str()
            .ok_or(AppCoreError::InvalidWorkspaceSelection)?
            .to_owned();
        let display_name = Path::new(&canonical_root)
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or(&canonical_root)
            .to_owned();

        let grant = self.with_storage(|storage| {
            let existing = if reuse_existing {
                storage.workspace_grant_for_root(&canonical_root)?
            } else {
                None
            };
            let grant = WorkspaceGrant {
                id: existing.as_ref().map_or_else(
                    || WorkspaceGrantId(uuid::Uuid::now_v7().to_string()),
                    |item| item.id.clone(),
                ),
                display_name,
                canonical_root,
                bookmark_data,
                permission_level: existing
                    .as_ref()
                    .map_or(WorkspacePermissionLevel::ReadOnly, |item| {
                        item.permission_level
                    }),
                created_at: existing.map_or_else(|| created_at.to_owned(), |item| item.created_at),
                revoked_at: None,
            };
            storage.save_workspace_grant(&grant)?;
            Ok(workspace_summary(&grant))
        })?;
        self.record_lifecycle_audit(
            AuditEventKind::WorkspaceGranted,
            Some(grant.id.0.clone()),
            created_at,
        );
        Ok(grant)
    }

    /// Lists frontend-safe metadata for every active workspace grant.
    ///
    /// # Errors
    ///
    /// Returns an error when persisted grant metadata cannot be loaded safely.
    pub fn workspace_grants(&self) -> Result<Vec<WorkspaceGrantSummary>, AppCoreError> {
        self.with_storage(|storage| {
            storage
                .workspace_grants()
                .map(|grants| grants.iter().map(workspace_summary).collect())
        })
    }

    /// Lists all projects for project-first navigation.
    ///
    /// # Errors
    ///
    /// Returns an error when local project metadata cannot be loaded.
    pub fn projects(&self) -> Result<Vec<ProjectSummary>, AppCoreError> {
        self.with_storage(|storage| {
            storage
                .projects()
                .map(|projects| projects.iter().map(project_summary).collect())
        })
    }

    /// Lists the active folder settings owned by one project.
    ///
    /// # Errors
    ///
    /// Returns an error when the project is absent or persistence fails.
    pub fn project_workspaces(
        &self,
        project_id: &ProjectId,
    ) -> Result<Vec<ProjectWorkspaceSummary>, AppCoreError> {
        self.with_storage(|storage| {
            if storage.project(project_id)?.is_none() {
                return Err(StorageError::InvalidRecord("project was not found"));
            }
            storage.project_workspaces(project_id).map(|workspaces| {
                workspaces
                    .iter()
                    .map(|workspace| ProjectWorkspaceSummary {
                        grant_id: workspace.grant.id.clone(),
                        display_name: workspace.grant.display_name.clone(),
                        role: workspace.role,
                        permission_level: workspace.grant.permission_level,
                        added_at: workspace.added_at.clone(),
                    })
                    .collect()
            })
        })
    }

    /// Creates a project independently of any folder grant.
    ///
    /// # Errors
    ///
    /// Returns an error when the name is invalid or persistence fails.
    pub fn create_project(
        &self,
        name: &str,
        created_at: &str,
    ) -> Result<ProjectSummary, AppCoreError> {
        if name.is_empty() || name.trim() != name || name.len() > MAX_PROJECT_NAME_BYTES {
            return Err(AppCoreError::InvalidProjectName);
        }
        let project = Project {
            id: ProjectId(uuid::Uuid::now_v7().to_string()),
            name: name.to_owned(),
            created_at: created_at.to_owned(),
            updated_at: created_at.to_owned(),
        };
        self.with_storage(|storage| storage.save_project(&project))?;
        self.record_lifecycle_audit(
            AuditEventKind::ProjectCreated,
            Some(project.id.0.clone()),
            created_at,
        );
        Ok(project_summary(&project))
    }

    /// Grants and attaches an optional folder to an existing project.
    ///
    /// # Errors
    ///
    /// Returns an error when the project or selected folder is unavailable.
    pub fn add_project_workspace(
        &self,
        project_id: &ProjectId,
        selected_root: impl AsRef<Path>,
        bookmark_data: Vec<u8>,
        added_at: &str,
    ) -> Result<ProjectWorkspaceSummary, AppCoreError> {
        if self
            .with_storage(|storage| storage.project(project_id))?
            .is_none()
        {
            return Err(AppCoreError::ProjectNotFound);
        }
        let canonical_root = fs::canonicalize(selected_root.as_ref())?;
        let canonical_root_text = canonical_root
            .to_str()
            .ok_or(AppCoreError::InvalidWorkspaceSelection)?;
        let duplicate = self.with_storage(|storage| {
            storage.project_workspaces(project_id).map(|workspaces| {
                workspaces
                    .iter()
                    .any(|workspace| workspace.grant.canonical_root == canonical_root_text)
            })
        })?;
        if duplicate {
            return Err(AppCoreError::InvalidWorkspaceSelection);
        }
        let grant = self.grant_workspace_internal(selected_root, bookmark_data, added_at, false)?;
        let role = if self.project_workspaces(project_id)?.is_empty() {
            ProjectWorkspaceRole::Primary
        } else {
            ProjectWorkspaceRole::Additional
        };
        self.with_storage(|storage| {
            storage.attach_project_workspace(project_id, &grant.id, role, added_at)
        })?;
        Ok(ProjectWorkspaceSummary {
            grant_id: grant.id,
            display_name: grant.display_name,
            role,
            permission_level: grant.permission_level,
            added_at: added_at.to_owned(),
        })
    }

    /// Detaches one folder from a project and revokes an otherwise unused grant.
    ///
    /// # Errors
    ///
    /// Returns an error when persistence fails.
    pub fn remove_project_workspace(
        &self,
        project_id: &ProjectId,
        grant_id: &WorkspaceGrantId,
        removed_at: &str,
    ) -> Result<bool, AppCoreError> {
        let _workspace_guard = self.write_workspace_operations()?;
        let (detached, revoked) = self.with_storage(|storage| {
            let detached = storage.detach_project_workspace(project_id, grant_id, removed_at)?;
            let revoked = detached
                && storage.workspace_project_count(grant_id)? == 0
                && storage.revoke_workspace_grant(grant_id, removed_at)?;
            Ok((detached, revoked))
        })?;
        if revoked {
            self.record_lifecycle_audit(
                AuditEventKind::WorkspaceRevoked,
                Some(grant_id.0.clone()),
                removed_at,
            );
        }
        Ok(detached)
    }

    /// Changes one project folder's broker-enforced permission.
    ///
    /// # Errors
    ///
    /// Returns an error when the folder does not belong to the project.
    pub fn update_project_workspace_permission(
        &self,
        project_id: &ProjectId,
        grant_id: &WorkspaceGrantId,
        permission_level: WorkspacePermissionLevel,
    ) -> Result<ProjectWorkspaceSummary, AppCoreError> {
        let _workspace_guard = self.write_workspace_operations()?;
        let belongs =
            self.with_storage(|storage| storage.project_has_workspace(project_id, grant_id))?;
        if !belongs {
            return Err(AppCoreError::WorkspaceGrantNotFound);
        }
        let grant = self
            .with_storage(|storage| {
                storage.update_workspace_permission(grant_id, permission_level)
            })?
            .ok_or(AppCoreError::WorkspaceGrantNotFound)?;
        self.record_lifecycle_audit_now(
            AuditEventKind::WorkspacePermissionChanged,
            Some(grant_id.0.clone()),
        );
        let workspace = self
            .project_workspaces(project_id)?
            .into_iter()
            .find(|workspace| workspace.grant_id == grant.id)
            .ok_or(AppCoreError::WorkspaceGrantNotFound)?;
        Ok(workspace)
    }

    /// Revokes one workspace capability and erases its persisted bookmark.
    ///
    /// # Errors
    ///
    /// Returns an error when the timestamp is invalid or persistence fails.
    pub fn revoke_workspace(
        &self,
        grant_id: &WorkspaceGrantId,
        revoked_at: &str,
    ) -> Result<bool, AppCoreError> {
        let _workspace_guard = self.write_workspace_operations()?;
        let revoked =
            self.with_storage(|storage| storage.revoke_workspace_grant(grant_id, revoked_at))?;
        if revoked {
            self.record_lifecycle_audit(
                AuditEventKind::WorkspaceRevoked,
                Some(grant_id.0.clone()),
                revoked_at,
            );
        }
        Ok(revoked)
    }

    /// Changes the permission level of one active workspace grant.
    ///
    /// # Errors
    ///
    /// Returns an error when the grant is unavailable or persistence fails.
    pub fn update_workspace_permission(
        &self,
        grant_id: &WorkspaceGrantId,
        permission_level: WorkspacePermissionLevel,
    ) -> Result<WorkspaceGrantSummary, AppCoreError> {
        let _workspace_guard = self.write_workspace_operations()?;
        let grant = self
            .with_storage(|storage| {
                storage.update_workspace_permission(grant_id, permission_level)
            })?
            .as_ref()
            .map(workspace_summary)
            .ok_or(AppCoreError::WorkspaceGrantNotFound)?;
        self.record_lifecycle_audit_now(
            AuditEventKind::WorkspacePermissionChanged,
            Some(grant_id.0.clone()),
        );
        Ok(grant)
    }

    /// Authorizes one operation using the active grant's current persisted level.
    ///
    /// # Errors
    ///
    /// Returns an error when the grant is unavailable or the filesystem broker
    /// rejects the operation or approval state.
    pub fn authorize_workspace_operation(
        &self,
        grant_id: &WorkspaceGrantId,
        operation: WorkspaceOperation,
    ) -> Result<(), AppCoreError> {
        let _workspace_guard = self.read_workspace_operations()?;
        let permission = self
            .with_storage(|storage| storage.workspace_grant(grant_id))?
            .map(|grant| grant.permission_level)
            .ok_or(AppCoreError::WorkspaceGrantNotFound)?;
        filesystem_tools::authorize_workspace_operation(permission, operation)
            .map_err(AppCoreError::WorkspaceAuthorization)
    }

    /// Returns a copy of an active grant's opaque bookmark for the native shell.
    ///
    /// This authority-bearing value must never be serialized across IPC. The
    /// native shell resolves and activates it before calling a filesystem method.
    ///
    /// # Errors
    ///
    /// Returns an error when the grant is revoked, missing or unavailable.
    pub fn workspace_bookmark_data(
        &self,
        grant_id: &WorkspaceGrantId,
    ) -> Result<Vec<u8>, AppCoreError> {
        let _workspace_guard = self.read_workspace_operations()?;
        self.with_storage(|storage| storage.workspace_grant(grant_id))?
            .map(|grant| grant.bookmark_data)
            .filter(|bookmark| !bookmark.is_empty())
            .ok_or(AppCoreError::WorkspaceGrantNotFound)
    }

    /// Returns an active bookmark only when it is attached to the named project.
    ///
    /// # Errors
    ///
    /// Returns an error when the project-folder association or grant is unavailable.
    pub fn project_workspace_bookmark_data(
        &self,
        project_id: &ProjectId,
        grant_id: &WorkspaceGrantId,
    ) -> Result<Vec<u8>, AppCoreError> {
        let _workspace_guard = self.read_workspace_operations()?;
        let belongs =
            self.with_storage(|storage| storage.project_has_workspace(project_id, grant_id))?;
        if !belongs {
            return Err(AppCoreError::WorkspaceGrantNotFound);
        }
        self.with_storage(|storage| storage.workspace_grant(grant_id))?
            .map(|grant| grant.bookmark_data)
            .filter(|bookmark| !bookmark.is_empty())
            .ok_or(AppCoreError::WorkspaceGrantNotFound)
    }
}
