use crate::{
    AppCore, AppCoreError, AssetReference, AuditEvent, AuditEventKind, AuditFilesystemTool,
    AuditOutcome, AuditPermissionDecision, FilesystemCreateDirectoryResult, FilesystemFindPathKind,
    FilesystemFindPathQuery, FilesystemFindPathsResult, FilesystemListResult, FilesystemMetadata,
    FilesystemReadTextResult, FilesystemScanResult, FilesystemSearchResult,
    FilesystemTextReplacement, FilesystemToolError, FilesystemUndoChange, FilesystemUndoResult,
    FilesystemUndoStatus, FilesystemWriteResult, FilesystemWriteTextChange, MAX_IMAGE_IMPORT_BYTES,
    OffsetDateTime, Path, Rfc3339, WorkspaceGrantId, WorkspaceOperation,
    filesystem_find_paths_result, filesystem_list_result, filesystem_metadata,
    filesystem_read_text_result, filesystem_scan_result, filesystem_search_result,
    filesystem_undo_result, filesystem_write_result,
};

impl AppCore {
    /// Executes bounded `filesystem.list` under an active read capability.
    ///
    /// # Errors
    ///
    /// Returns an error when the grant or requested directory fails closed.
    pub fn filesystem_list(
        &self,
        grant_id: &WorkspaceGrantId,
        resolved_root: impl AsRef<Path>,
        relative_path: &str,
    ) -> Result<FilesystemListResult, AppCoreError> {
        let _workspace_guard = self.read_workspace_operations()?;
        let operation_id = self.authorize_audited_filesystem(
            AuditFilesystemTool::List,
            grant_id,
            WorkspaceOperation::Read,
        )?;
        let result = self
            .open_workspace(resolved_root)
            .map_err(FilesystemToolError::from)
            .and_then(|workspace| filesystem_tools::list(&workspace, relative_path))
            .map(filesystem_list_result)
            .map_err(Into::into);
        match &result {
            Ok(listing) => self.record_filesystem_audit(
                &operation_id,
                AuditFilesystemTool::List,
                grant_id,
                Some(relative_path),
                AuditPermissionDecision::Allowed,
                AuditOutcome::Succeeded,
                Some(listing.entries.len()),
                Some(listing.truncated),
            ),
            Err(_) => self.record_filesystem_audit(
                &operation_id,
                AuditFilesystemTool::List,
                grant_id,
                None,
                AuditPermissionDecision::Allowed,
                AuditOutcome::Failed,
                None,
                None,
            ),
        }
        result
    }

    /// Executes bounded recursive filename discovery under an active read capability.
    ///
    /// # Errors
    ///
    /// Returns an error when the grant, filters or starting directories fail closed.
    pub fn filesystem_find_paths(
        &self,
        grant_id: &WorkspaceGrantId,
        resolved_root: impl AsRef<Path>,
        queries: &[FilesystemFindPathQuery],
    ) -> Result<FilesystemFindPathsResult, AppCoreError> {
        let _workspace_guard = self.read_workspace_operations()?;
        let operation_id = self.authorize_audited_filesystem(
            AuditFilesystemTool::FindPaths,
            grant_id,
            WorkspaceOperation::Read,
        )?;
        let broker_queries = queries
            .iter()
            .map(|query| filesystem_tools::FilesystemFindPathQuery {
                relative_path: query.relative_path.clone(),
                name_contains: query.name_contains.clone(),
                extensions: query.extensions.clone(),
                kind: match query.kind {
                    FilesystemFindPathKind::Any => filesystem_tools::FilesystemFindPathKind::Any,
                    FilesystemFindPathKind::File => filesystem_tools::FilesystemFindPathKind::File,
                    FilesystemFindPathKind::Directory => {
                        filesystem_tools::FilesystemFindPathKind::Directory
                    }
                },
                max_depth: query.max_depth,
            })
            .collect::<Vec<_>>();
        let result = self
            .open_workspace(resolved_root)
            .map_err(FilesystemToolError::from)
            .and_then(|workspace| filesystem_tools::find_paths(&workspace, &broker_queries))
            .map(filesystem_find_paths_result)
            .map_err(Into::into);
        match &result {
            Ok(found) => self.record_filesystem_audit(
                &operation_id,
                AuditFilesystemTool::FindPaths,
                grant_id,
                None,
                AuditPermissionDecision::Allowed,
                AuditOutcome::Succeeded,
                Some(
                    found
                        .results
                        .iter()
                        .map(|result| result.matches.len())
                        .sum(),
                ),
                Some(found.results.iter().any(|result| result.truncated)),
            ),
            Err(_) => self.record_failed_filesystem_audit(
                &operation_id,
                AuditFilesystemTool::FindPaths,
                grant_id,
            ),
        }
        result
    }

    /// Executes bounded `filesystem.stat` under an active read capability.
    ///
    /// # Errors
    ///
    /// Returns an error when the grant or requested entry fails closed.
    pub fn filesystem_stat(
        &self,
        grant_id: &WorkspaceGrantId,
        resolved_root: impl AsRef<Path>,
        relative_path: &str,
    ) -> Result<FilesystemMetadata, AppCoreError> {
        let _workspace_guard = self.read_workspace_operations()?;
        let operation_id = self.authorize_audited_filesystem(
            AuditFilesystemTool::Stat,
            grant_id,
            WorkspaceOperation::Read,
        )?;
        let result = self
            .open_workspace(resolved_root)
            .map_err(FilesystemToolError::from)
            .and_then(|workspace| filesystem_tools::stat(&workspace, relative_path))
            .map(filesystem_metadata)
            .map_err(Into::into);
        match &result {
            Ok(metadata) => self.record_filesystem_audit(
                &operation_id,
                AuditFilesystemTool::Stat,
                grant_id,
                Some(&metadata.relative_path),
                AuditPermissionDecision::Allowed,
                AuditOutcome::Succeeded,
                Some(1),
                None,
            ),
            Err(_) => self.record_failed_filesystem_audit(
                &operation_id,
                AuditFilesystemTool::Stat,
                grant_id,
            ),
        }
        result
    }

    /// Executes bounded `filesystem.read_text` under an active read capability.
    ///
    /// # Errors
    ///
    /// Returns an error when the grant or requested text file fails closed.
    pub fn filesystem_read_text(
        &self,
        grant_id: &WorkspaceGrantId,
        resolved_root: impl AsRef<Path>,
        relative_path: &str,
    ) -> Result<FilesystemReadTextResult, AppCoreError> {
        let _workspace_guard = self.read_workspace_operations()?;
        let operation_id = self.authorize_audited_filesystem(
            AuditFilesystemTool::ReadText,
            grant_id,
            WorkspaceOperation::Read,
        )?;
        let result = self
            .open_workspace(resolved_root)
            .map_err(FilesystemToolError::from)
            .and_then(|workspace| filesystem_tools::read_text(&workspace, relative_path))
            .map(filesystem_read_text_result)
            .map_err(Into::into);
        match &result {
            Ok(read) => self.record_filesystem_audit(
                &operation_id,
                AuditFilesystemTool::ReadText,
                grant_id,
                Some(&read.relative_path),
                AuditPermissionDecision::Allowed,
                AuditOutcome::Succeeded,
                Some(1),
                None,
            ),
            Err(_) => self.record_failed_filesystem_audit(
                &operation_id,
                AuditFilesystemTool::ReadText,
                grant_id,
            ),
        }
        result
    }

    /// Imports one bounded workspace image through the managed media pipeline.
    ///
    /// The workspace path is used only by the Rust capability broker. The
    /// returned asset contains opaque managed-file references and normalized
    /// metadata, never the source path or bookmark capability.
    ///
    /// # Errors
    ///
    /// Returns an error when authorization, safe path resolution, stable file
    /// reading, image validation, normalization, or managed persistence fails.
    pub fn filesystem_inspect_image(
        &self,
        grant_id: &WorkspaceGrantId,
        resolved_root: impl AsRef<Path>,
        relative_path: &str,
        imported_at: &str,
    ) -> Result<AssetReference, AppCoreError> {
        let _workspace_guard = self.read_workspace_operations()?;
        let operation_id = self.authorize_audited_filesystem(
            AuditFilesystemTool::InspectImage,
            grant_id,
            WorkspaceOperation::Read,
        )?;
        let workspace = match self
            .open_workspace(resolved_root)
            .map_err(FilesystemToolError::from)
        {
            Ok(workspace) => workspace,
            Err(error) => {
                self.record_failed_filesystem_audit(
                    &operation_id,
                    AuditFilesystemTool::InspectImage,
                    grant_id,
                );
                return Err(error.into());
            }
        };
        let read =
            match filesystem_tools::read_bytes(&workspace, relative_path, MAX_IMAGE_IMPORT_BYTES) {
                Ok(read) => read,
                Err(error) => {
                    self.record_failed_filesystem_audit(
                        &operation_id,
                        AuditFilesystemTool::InspectImage,
                        grant_id,
                    );
                    return Err(error.into());
                }
            };
        let result = self.import_image(&read.bytes, imported_at);
        self.record_filesystem_audit(
            &operation_id,
            AuditFilesystemTool::InspectImage,
            grant_id,
            Some(&read.relative_path),
            AuditPermissionDecision::Allowed,
            if result.is_ok() {
                AuditOutcome::Succeeded
            } else {
                AuditOutcome::Failed
            },
            result.as_ref().ok().map(|_| 1),
            None,
        );
        result
    }

    /// Executes bounded literal `filesystem.search` under an active read capability.
    ///
    /// # Errors
    ///
    /// Returns an error when the grant, query or starting directory fails closed.
    pub fn filesystem_search(
        &self,
        grant_id: &WorkspaceGrantId,
        resolved_root: impl AsRef<Path>,
        relative_path: &str,
        query: &str,
    ) -> Result<FilesystemSearchResult, AppCoreError> {
        let _workspace_guard = self.read_workspace_operations()?;
        let operation_id = self.authorize_audited_filesystem(
            AuditFilesystemTool::Search,
            grant_id,
            WorkspaceOperation::Read,
        )?;
        let result = self
            .open_workspace(resolved_root)
            .map_err(FilesystemToolError::from)
            .and_then(|workspace| filesystem_tools::search(&workspace, relative_path, query))
            .map(filesystem_search_result)
            .map_err(Into::into);
        match &result {
            Ok(search) => self.record_filesystem_audit(
                &operation_id,
                AuditFilesystemTool::Search,
                grant_id,
                Some(relative_path),
                AuditPermissionDecision::Allowed,
                AuditOutcome::Succeeded,
                Some(search.matches.len()),
                Some(search.truncated),
            ),
            Err(_) => self.record_failed_filesystem_audit(
                &operation_id,
                AuditFilesystemTool::Search,
                grant_id,
            ),
        }
        result
    }

    /// Executes bounded aggregate `filesystem.scan` under an active read capability.
    ///
    /// # Errors
    ///
    /// Returns an error when the grant, filters or starting directory fail closed.
    pub fn filesystem_scan(
        &self,
        grant_id: &WorkspaceGrantId,
        resolved_root: impl AsRef<Path>,
        relative_path: &str,
        recursive: bool,
        minimum_line_count: usize,
        excluded_extensions: &[String],
    ) -> Result<FilesystemScanResult, AppCoreError> {
        let _workspace_guard = self.read_workspace_operations()?;
        let operation_id = self.authorize_audited_filesystem(
            AuditFilesystemTool::Scan,
            grant_id,
            WorkspaceOperation::Read,
        )?;
        let result = self
            .open_workspace(resolved_root)
            .map_err(FilesystemToolError::from)
            .and_then(|workspace| {
                filesystem_tools::scan(
                    &workspace,
                    relative_path,
                    recursive,
                    minimum_line_count,
                    excluded_extensions,
                )
            })
            .map(filesystem_scan_result)
            .map_err(Into::into);
        match &result {
            Ok(scan) => self.record_filesystem_audit(
                &operation_id,
                AuditFilesystemTool::Scan,
                grant_id,
                Some(relative_path),
                AuditPermissionDecision::Allowed,
                AuditOutcome::Succeeded,
                Some(scan.matches.len()),
                Some(scan.truncated),
            ),
            Err(_) => self.record_failed_filesystem_audit(
                &operation_id,
                AuditFilesystemTool::Scan,
                grant_id,
            ),
        }
        result
    }

    /// Applies bounded digest-checked direct text replacements under write permission.
    ///
    /// # Errors
    ///
    /// Returns an error when authorization, path safety, original content,
    /// limits, staging or atomic replacement fails closed.
    pub fn filesystem_write_text(
        &self,
        grant_id: &WorkspaceGrantId,
        resolved_root: impl AsRef<Path>,
        changes: &[FilesystemWriteTextChange],
    ) -> Result<FilesystemWriteResult, AppCoreError> {
        let _workspace_guard = self.write_workspace_operations()?;
        let operation_id = self.authorize_audited_filesystem(
            AuditFilesystemTool::WriteText,
            grant_id,
            WorkspaceOperation::Write,
        )?;
        let workspace = match self
            .open_workspace(resolved_root)
            .map_err(FilesystemToolError::from)
        {
            Ok(workspace) => workspace,
            Err(error) => {
                self.record_failed_filesystem_audit(
                    &operation_id,
                    AuditFilesystemTool::WriteText,
                    grant_id,
                );
                return Err(error.into());
            }
        };
        let changes = changes
            .iter()
            .map(|change| filesystem_tools::FilesystemWriteChange {
                relative_path: change.relative_path.clone(),
                expected_sha256: change.expected_sha256.clone(),
                content: change.content.clone(),
                create_missing_parents: change.create_missing_parents,
            })
            .collect::<Vec<_>>();
        let result = filesystem_tools::write_text(&workspace, &changes)
            .map(filesystem_write_result)
            .map_err(Into::into);
        match &result {
            Ok(write) => {
                for change in &write.changes {
                    self.record_filesystem_audit(
                        &operation_id,
                        AuditFilesystemTool::WriteText,
                        grant_id,
                        Some(&change.relative_path),
                        AuditPermissionDecision::Allowed,
                        AuditOutcome::Succeeded,
                        Some(1),
                        None,
                    );
                }
            }
            Err(_) => self.record_failed_filesystem_audit(
                &operation_id,
                AuditFilesystemTool::WriteText,
                grant_id,
            ),
        }
        result
    }

    /// Creates exactly one directory under an existing parent with write permission.
    ///
    /// # Errors
    ///
    /// Returns an error when the grant is not writable, the relative path is
    /// unsafe, the parent is missing, the target exists, or creation fails.
    pub fn filesystem_create_directory(
        &self,
        grant_id: &WorkspaceGrantId,
        resolved_root: impl AsRef<Path>,
        relative_path: &str,
    ) -> Result<FilesystemCreateDirectoryResult, AppCoreError> {
        let _workspace_guard = self.write_workspace_operations()?;
        let operation_id = self.authorize_audited_filesystem(
            AuditFilesystemTool::CreateDirectory,
            grant_id,
            WorkspaceOperation::Write,
        )?;
        let workspace = match self
            .open_workspace(resolved_root)
            .map_err(FilesystemToolError::from)
        {
            Ok(workspace) => workspace,
            Err(error) => {
                self.record_failed_filesystem_audit(
                    &operation_id,
                    AuditFilesystemTool::CreateDirectory,
                    grant_id,
                );
                return Err(error.into());
            }
        };
        let result = filesystem_tools::create_directory(&workspace, relative_path)
            .map_err(AppCoreError::from);
        match &result {
            Ok(created) => self.record_filesystem_audit(
                &operation_id,
                AuditFilesystemTool::CreateDirectory,
                grant_id,
                Some(&created.relative_path),
                AuditPermissionDecision::Allowed,
                AuditOutcome::Succeeded,
                Some(1),
                None,
            ),
            Err(_) => self.record_failed_filesystem_audit(
                &operation_id,
                AuditFilesystemTool::CreateDirectory,
                grant_id,
            ),
        }
        result
    }

    /// Applies exact substitutions without model-side full-file re-emission.
    ///
    /// # Errors
    ///
    /// Returns an error when authorization, path safety, digest, occurrence
    /// counts, limits, or atomic replacement fail closed.
    pub fn filesystem_replace_text(
        &self,
        grant_id: &WorkspaceGrantId,
        resolved_root: impl AsRef<Path>,
        relative_path: &str,
        expected_sha256: &str,
        replacements: &[FilesystemTextReplacement],
    ) -> Result<FilesystemWriteResult, AppCoreError> {
        let _workspace_guard = self.write_workspace_operations()?;
        let operation_id = self.authorize_audited_filesystem(
            AuditFilesystemTool::ReplaceText,
            grant_id,
            WorkspaceOperation::Write,
        )?;
        let workspace = match self
            .open_workspace(resolved_root)
            .map_err(FilesystemToolError::from)
        {
            Ok(workspace) => workspace,
            Err(error) => {
                self.record_failed_filesystem_audit(
                    &operation_id,
                    AuditFilesystemTool::ReplaceText,
                    grant_id,
                );
                return Err(error.into());
            }
        };
        let replacements = replacements
            .iter()
            .map(|replacement| filesystem_tools::FilesystemTextReplacement {
                old_text: replacement.old_text.clone(),
                new_text: replacement.new_text.clone(),
                expected_occurrences: replacement.expected_occurrences,
            })
            .collect::<Vec<_>>();
        let result = filesystem_tools::replace_text(
            &workspace,
            relative_path,
            expected_sha256,
            &replacements,
        )
        .map(filesystem_write_result)
        .map_err(Into::into);
        match &result {
            Ok(write) => {
                for change in &write.changes {
                    self.record_filesystem_audit(
                        &operation_id,
                        AuditFilesystemTool::ReplaceText,
                        grant_id,
                        Some(&change.relative_path),
                        AuditPermissionDecision::Allowed,
                        AuditOutcome::Succeeded,
                        Some(1),
                        None,
                    );
                }
            }
            Err(_) => self.record_failed_filesystem_audit(
                &operation_id,
                AuditFilesystemTool::ReplaceText,
                grant_id,
            ),
        }
        result
    }

    /// Attempts an explicit digest-checked undo and reports every file outcome.
    ///
    /// # Errors
    ///
    /// Returns an error when authorization or the workspace capability is unavailable.
    pub fn filesystem_undo_text(
        &self,
        grant_id: &WorkspaceGrantId,
        resolved_root: impl AsRef<Path>,
        changes: &[FilesystemUndoChange],
    ) -> Result<FilesystemUndoResult, AppCoreError> {
        let _workspace_guard = self.write_workspace_operations()?;
        let operation_id = self.authorize_audited_filesystem(
            AuditFilesystemTool::UndoText,
            grant_id,
            WorkspaceOperation::Write,
        )?;
        let workspace = match self
            .open_workspace(resolved_root)
            .map_err(FilesystemToolError::from)
        {
            Ok(workspace) => workspace,
            Err(error) => {
                self.record_failed_filesystem_audit(
                    &operation_id,
                    AuditFilesystemTool::UndoText,
                    grant_id,
                );
                return Err(error.into());
            }
        };
        let result = filesystem_undo_result(filesystem_tools::undo_text(&workspace, changes));
        for file in &result.files {
            let outcome = match file.status {
                FilesystemUndoStatus::Undone => AuditOutcome::Succeeded,
                FilesystemUndoStatus::Conflict | FilesystemUndoStatus::Failed => {
                    AuditOutcome::Failed
                }
            };
            self.record_filesystem_audit(
                &operation_id,
                AuditFilesystemTool::UndoText,
                grant_id,
                Some(&file.relative_path),
                AuditPermissionDecision::Allowed,
                outcome,
                Some(1),
                None,
            );
        }
        Ok(result)
    }

    /// Checks whether an undo is currently safe under write permission.
    ///
    /// # Errors
    ///
    /// Returns an error when authorization or the workspace capability is unavailable.
    pub fn filesystem_undo_available(
        &self,
        grant_id: &WorkspaceGrantId,
        resolved_root: impl AsRef<Path>,
        changes: &[FilesystemUndoChange],
    ) -> Result<bool, AppCoreError> {
        let _workspace_guard = self.read_workspace_operations()?;
        let operation_id = self.authorize_audited_filesystem(
            AuditFilesystemTool::UndoAvailable,
            grant_id,
            WorkspaceOperation::Write,
        )?;
        let workspace = match self
            .open_workspace(resolved_root)
            .map_err(FilesystemToolError::from)
        {
            Ok(workspace) => workspace,
            Err(error) => {
                self.record_failed_filesystem_audit(
                    &operation_id,
                    AuditFilesystemTool::UndoAvailable,
                    grant_id,
                );
                return Err(error.into());
            }
        };
        let available = filesystem_tools::undo_available(&workspace, changes);
        self.record_filesystem_audit(
            &operation_id,
            AuditFilesystemTool::UndoAvailable,
            grant_id,
            None,
            AuditPermissionDecision::Allowed,
            AuditOutcome::Succeeded,
            Some(usize::from(available)),
            None,
        );
        Ok(available)
    }

    pub(crate) fn read_workspace_operations(
        &self,
    ) -> Result<std::sync::RwLockReadGuard<'_, ()>, AppCoreError> {
        self.workspace_operations
            .read()
            .map_err(|_| AppCoreError::WorkspaceOperationsUnavailable)
    }

    pub(crate) fn write_workspace_operations(
        &self,
    ) -> Result<std::sync::RwLockWriteGuard<'_, ()>, AppCoreError> {
        self.workspace_operations
            .write()
            .map_err(|_| AppCoreError::WorkspaceOperationsUnavailable)
    }

    fn authorize_audited_filesystem(
        &self,
        tool: AuditFilesystemTool,
        grant_id: &WorkspaceGrantId,
        operation: WorkspaceOperation,
    ) -> Result<String, AppCoreError> {
        let operation_id = uuid::Uuid::now_v7().to_string();
        let authorization = match operation {
            WorkspaceOperation::Read => self.authorize_active_workspace_read(grant_id),
            WorkspaceOperation::Write => self.authorize_active_workspace_write(grant_id),
        };
        if let Err(error) = &authorization {
            let denied = matches!(
                error,
                AppCoreError::WorkspaceGrantNotFound | AppCoreError::WorkspaceAuthorization(_)
            );
            self.record_filesystem_audit(
                &operation_id,
                tool,
                grant_id,
                None,
                if denied {
                    AuditPermissionDecision::Denied
                } else {
                    AuditPermissionDecision::NotApplicable
                },
                if denied {
                    AuditOutcome::Denied
                } else {
                    AuditOutcome::Failed
                },
                None,
                None,
            );
        }
        authorization.map(|()| operation_id)
    }

    fn record_failed_filesystem_audit(
        &self,
        operation_id: &str,
        tool: AuditFilesystemTool,
        grant_id: &WorkspaceGrantId,
    ) {
        self.record_filesystem_audit(
            operation_id,
            tool,
            grant_id,
            None,
            AuditPermissionDecision::Allowed,
            AuditOutcome::Failed,
            None,
            None,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn record_filesystem_audit(
        &self,
        operation_id: &str,
        tool: AuditFilesystemTool,
        grant_id: &WorkspaceGrantId,
        relative_path: Option<&str>,
        permission_decision: AuditPermissionDecision,
        outcome: AuditOutcome,
        result_count: Option<usize>,
        truncated: Option<bool>,
    ) {
        self.record_audit_event(&AuditEvent {
            id: uuid::Uuid::now_v7().to_string(),
            kind: AuditEventKind::FilesystemTool,
            outcome,
            subject_id: None,
            operation_id: Some(operation_id.to_owned()),
            tool: Some(tool),
            workspace_grant_id: Some(grant_id.clone()),
            relative_path: relative_path.map(str::to_owned),
            permission_decision,
            result_count: result_count.map(|count| u32::try_from(count).unwrap_or(u32::MAX)),
            truncated,
            occurred_at: current_audit_timestamp(),
        });
    }

    pub(crate) fn record_lifecycle_audit_now(
        &self,
        kind: AuditEventKind,
        subject_id: Option<String>,
    ) {
        self.record_lifecycle_audit(kind, subject_id, &current_audit_timestamp());
    }

    pub(crate) fn record_lifecycle_audit(
        &self,
        kind: AuditEventKind,
        subject_id: Option<String>,
        occurred_at: &str,
    ) {
        self.record_audit_event(&AuditEvent {
            id: uuid::Uuid::now_v7().to_string(),
            kind,
            outcome: AuditOutcome::Succeeded,
            subject_id,
            operation_id: None,
            tool: None,
            workspace_grant_id: None,
            relative_path: None,
            permission_decision: AuditPermissionDecision::NotApplicable,
            result_count: None,
            truncated: None,
            occurred_at: occurred_at.to_owned(),
        });
    }

    pub(crate) fn record_audit_event(&self, event: &AuditEvent) {
        // Auditing observes the result; its failure must never alter the
        // authorization decision or filesystem effect being recorded.
        let _ = self.with_storage(|storage| storage.append_audit_event(event));
    }

    fn authorize_active_workspace_read(
        &self,
        grant_id: &WorkspaceGrantId,
    ) -> Result<(), AppCoreError> {
        let permission = self
            .with_storage(|storage| storage.workspace_grant(grant_id))?
            .map(|grant| grant.permission_level)
            .ok_or(AppCoreError::WorkspaceGrantNotFound)?;
        filesystem_tools::authorize_workspace_operation(permission, WorkspaceOperation::Read)
            .map_err(AppCoreError::WorkspaceAuthorization)
    }

    fn authorize_active_workspace_write(
        &self,
        grant_id: &WorkspaceGrantId,
    ) -> Result<(), AppCoreError> {
        let permission = self
            .with_storage(|storage| storage.workspace_grant(grant_id))?
            .map(|grant| grant.permission_level)
            .ok_or(AppCoreError::WorkspaceGrantNotFound)?;
        filesystem_tools::authorize_workspace_operation(permission, WorkspaceOperation::Write)
            .map_err(AppCoreError::WorkspaceAuthorization)
    }
}

pub(crate) fn current_audit_timestamp() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}
