//! Handle-relative filesystem operations inside user-granted roots.

mod directory_tools;
mod find_paths;
mod path_policy;
mod private_directory;
mod read_tools;
mod safe_path;
mod write_tools;

pub use directory_tools::{
    FilesystemCreateDirectoryError, FilesystemCreateDirectoryResult, create_directory,
};
pub use find_paths::{
    FilesystemFindPathKind, FilesystemFindPathQuery, FilesystemFindPathQueryResult,
    FilesystemFindPathsResult, MAX_FIND_PATH_DEPTH, MAX_FIND_PATH_DIRECTORIES,
    MAX_FIND_PATH_EXTENSIONS, MAX_FIND_PATH_MATCHES, MAX_FIND_PATH_NAME_BYTES,
    MAX_FIND_PATH_QUERIES, find_paths,
};
pub use permissions::{WorkspaceAuthorizationError, WorkspaceOperation, WorkspacePermissionLevel};
pub use private_directory::PrivateDirectory;
pub use read_tools::{
    FilesystemEntry, FilesystemEntryKind, FilesystemListResult, FilesystemMetadata,
    FilesystemReadBytesResult, FilesystemReadTextResult, FilesystemScanMatch, FilesystemScanResult,
    FilesystemSearchMatch, FilesystemSearchResult, FilesystemToolError, MAX_LIST_ENTRIES,
    MAX_READ_TEXT_BYTES, MAX_SCAN_DIRECTORIES, MAX_SCAN_FILE_BYTES, MAX_SCAN_FILES,
    MAX_SCAN_MATCHES, MAX_SCAN_TOTAL_BYTES, MAX_SEARCH_DIRECTORIES, MAX_SEARCH_FILE_BYTES,
    MAX_SEARCH_FILES, MAX_SEARCH_MATCHES, MAX_SEARCH_QUERY_BYTES, MAX_SEARCH_TOTAL_BYTES, list,
    read_bytes, read_text, scan, search, stat,
};
pub use safe_path::{SafePathError, WorkspaceDirectory, WorkspaceIdentity};
pub use write_tools::{
    FilesystemAppliedChange, FilesystemTextReplacement, FilesystemUndoChange,
    FilesystemUndoFileResult, FilesystemUndoResult, FilesystemUndoStatus, FilesystemWriteChange,
    FilesystemWriteError, FilesystemWriteResult, MAX_TEXT_REPLACEMENT_BYTES,
    MAX_TEXT_REPLACEMENT_OCCURRENCES, MAX_TEXT_REPLACEMENTS, MAX_WRITE_FILE_BYTES, MAX_WRITE_FILES,
    MAX_WRITE_TOTAL_BYTES, replace_text, undo_available, undo_text, write_text,
};

/// Authorizes an operation at the Rust filesystem-broker boundary.
///
/// Future filesystem tools must pass this gate before resolving or accessing a
/// workspace path.
///
/// # Errors
///
/// Returns an error when the persisted grant level or approval state does not
/// authorize the requested operation.
pub const fn authorize_workspace_operation(
    permission: WorkspacePermissionLevel,
    operation: WorkspaceOperation,
) -> Result<(), WorkspaceAuthorizationError> {
    permissions::authorize_workspace_operation(permission, operation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broker_enforces_the_workspace_decision_table() {
        assert!(
            authorize_workspace_operation(
                WorkspacePermissionLevel::ReadOnly,
                WorkspaceOperation::Write,
            )
            .is_err()
        );
        assert!(
            authorize_workspace_operation(
                WorkspacePermissionLevel::ReadAndWrite,
                WorkspaceOperation::Write,
            )
            .is_ok()
        );
    }
}
