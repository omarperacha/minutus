use super::{ApplicationStore, AuditEvent, StorageError, params};

impl ApplicationStore {
    /// Appends one allow-listed non-secret audit event.
    ///
    /// # Errors
    ///
    /// Returns an error when the database write fails.
    pub fn append_audit_event(&self, event: &AuditEvent) -> Result<(), StorageError> {
        validate_audit_event(event)?;
        self.connection.execute(
            "INSERT INTO audit_events
                (id, event_kind, outcome, subject_id, operation_id, tool_name,
                 workspace_grant_id, relative_path, permission_decision,
                 result_count, truncated, occurred_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                event.id,
                event.kind.as_str(),
                event.outcome.as_str(),
                event.subject_id,
                event.operation_id,
                event.tool.map(crate::AuditFilesystemTool::as_str),
                event.workspace_grant_id.as_ref().map(|id| id.0.as_str()),
                event.relative_path,
                event.permission_decision.as_str(),
                event.result_count,
                event.truncated,
                event.occurred_at
            ],
        )?;
        Ok(())
    }

    /// Returns the number of durable audit events without exposing event payloads.
    ///
    /// # Errors
    ///
    /// Returns an error when the database query fails.
    pub fn audit_event_count(&self) -> Result<u64, StorageError> {
        let count = self
            .connection
            .query_row("SELECT count(*) FROM audit_events", [], |row| {
                row.get::<_, i64>(0)
            })?;
        u64::try_from(count).map_err(|_| StorageError::CorruptDatabase)
    }
}

fn validate_audit_event(event: &AuditEvent) -> Result<(), StorageError> {
    const MAX_AUDIT_ID_BYTES: usize = 128;
    const MAX_AUDIT_PATH_BYTES: usize = 1_024;
    const MAX_AUDIT_TIMESTAMP_BYTES: usize = 64;

    let valid_id = |value: &str| !value.is_empty() && value.len() <= MAX_AUDIT_ID_BYTES;
    if !valid_id(&event.id)
        || event
            .subject_id
            .as_deref()
            .is_some_and(|value| !valid_id(value))
        || event
            .operation_id
            .as_deref()
            .is_some_and(|value| !valid_id(value))
        || event
            .workspace_grant_id
            .as_ref()
            .is_some_and(|id| !valid_id(&id.0))
        || event.occurred_at.is_empty()
        || event.occurred_at.len() > MAX_AUDIT_TIMESTAMP_BYTES
    {
        return Err(StorageError::InvalidRecord("audit metadata is invalid"));
    }

    if let Some(path) = &event.relative_path {
        let safe_relative_path = !path.is_empty()
            && path.len() <= MAX_AUDIT_PATH_BYTES
            && !path.starts_with(['/', '\\'])
            && !path.contains('\0')
            && !path.split('/').any(|component| component == "..");
        if !safe_relative_path {
            return Err(StorageError::InvalidRecord(
                "audit path is not workspace-relative",
            ));
        }
    }

    let is_filesystem = event.kind == crate::AuditEventKind::FilesystemTool;
    if is_filesystem
        != (event.tool.is_some()
            && event.operation_id.is_some()
            && event.workspace_grant_id.is_some())
        || (!is_filesystem
            && (event.tool.is_some()
                || event.operation_id.is_some()
                || event.workspace_grant_id.is_some()
                || event.relative_path.is_some()
                || event.result_count.is_some()
                || event.truncated.is_some()
                || event.permission_decision != crate::AuditPermissionDecision::NotApplicable))
    {
        return Err(StorageError::InvalidRecord(
            "audit event fields do not match its kind",
        ));
    }
    Ok(())
}
