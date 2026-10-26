//! Capability grants and authorization decisions.

use std::{error::Error, fmt};

pub use ipc_types::WorkspacePermissionLevel;

/// Closed set of filesystem operations understood by the permission broker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkspaceOperation {
    /// Read bounded content inside the granted root.
    Read,
    /// Modify content inside the granted root.
    Write,
}

/// A workspace permission rejected the requested operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkspaceAuthorizationError;

impl fmt::Display for WorkspaceAuthorizationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the workspace permission does not allow this operation")
    }
}

impl Error for WorkspaceAuthorizationError {}

/// Applies the closed workspace permission decision table.
///
/// # Errors
///
/// Returns an error when the permission level does not include the requested
/// capability.
pub const fn authorize_workspace_operation(
    permission: WorkspacePermissionLevel,
    operation: WorkspaceOperation,
) -> Result<(), WorkspaceAuthorizationError> {
    match (permission, operation) {
        (
            WorkspacePermissionLevel::ReadOnly | WorkspacePermissionLevel::ReadAndWrite,
            WorkspaceOperation::Read,
        )
        | (WorkspacePermissionLevel::ReadAndWrite, WorkspaceOperation::Write) => Ok(()),
        _ => Err(WorkspaceAuthorizationError),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_can_only_read() {
        assert!(
            authorize_workspace_operation(
                WorkspacePermissionLevel::ReadOnly,
                WorkspaceOperation::Read,
            )
            .is_ok()
        );
        assert!(
            authorize_workspace_operation(
                WorkspacePermissionLevel::ReadOnly,
                WorkspaceOperation::Write,
            )
            .is_err()
        );
    }

    #[test]
    fn read_and_write_allows_direct_writes() {
        let permission = WorkspacePermissionLevel::ReadAndWrite;
        assert!(authorize_workspace_operation(permission, WorkspaceOperation::Write).is_ok());
    }
}
