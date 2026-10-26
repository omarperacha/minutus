//! Exact, non-recursive directory creation inside workspace capabilities.

use std::{error::Error, fmt};

use rustix::fs::{self, Mode};

use crate::{SafePathError, WorkspaceDirectory};

/// Verified result from creating one directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemCreateDirectoryResult {
    /// Strict path relative to the workspace root.
    pub relative_path: String,
    /// Whether the requested directory was newly created.
    pub created: bool,
    /// Whether the new entry was reopened and verified as a safe directory.
    pub post_create_verified: bool,
}

/// Typed failure from direct directory creation.
#[derive(Debug)]
pub enum FilesystemCreateDirectoryError {
    /// The requested path, its parent, or an existing target was unsafe.
    Path(SafePathError),
    /// The target already exists.
    Conflict,
    /// Creation or durability failed.
    Io(std::io::Error),
}

impl fmt::Display for FilesystemCreateDirectoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(error) => error.fmt(formatter),
            Self::Conflict => formatter.write_str("workspace directory already exists"),
            Self::Io(error) => write!(formatter, "workspace directory creation failed: {error}"),
        }
    }
}

impl Error for FilesystemCreateDirectoryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Path(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::Conflict => None,
        }
    }
}

/// Creates exactly one directory. All parent directories must already exist.
///
/// # Errors
///
/// Returns a typed error when the relative path is unsafe, the parent is
/// missing, the target exists, or the handle-relative create operation fails.
pub fn create_directory(
    workspace: &WorkspaceDirectory,
    relative_path: &str,
) -> Result<FilesystemCreateDirectoryResult, FilesystemCreateDirectoryError> {
    let target = workspace
        .resolve_new_directory_target(relative_path)
        .map_err(|error| match error {
            SafePathError::UnexpectedFileType => FilesystemCreateDirectoryError::Conflict,
            other => FilesystemCreateDirectoryError::Path(other),
        })?;
    fs::mkdirat(
        &target.parent,
        &target.name,
        Mode::from_bits_truncate(0o755),
    )
    .map_err(|error| FilesystemCreateDirectoryError::Io(error.into()))?;
    let parent_synced = fs::fsync(&target.parent).is_ok();
    let post_create_verified = parent_synced && workspace.open_directory(relative_path).is_ok();
    Ok(FilesystemCreateDirectoryResult {
        relative_path: relative_path.to_owned(),
        created: true,
        post_create_verified,
    })
}

#[cfg(test)]
mod tests {
    use std::{error::Error, fs};

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn creates_one_exact_child_and_verifies_it() -> Result<(), Box<dyn Error>> {
        let root = tempdir()?;
        fs::create_dir(root.path().join("outputs"))?;
        let workspace = WorkspaceDirectory::open(root.path())?;
        let result = create_directory(&workspace, "outputs/monthly cashflow")?;
        assert!(result.created);
        assert!(result.post_create_verified);
        assert!(root.path().join("outputs/monthly cashflow").is_dir());
        Ok(())
    }

    #[test]
    fn refuses_existing_targets_missing_parents_and_traversal() -> Result<(), Box<dyn Error>> {
        let root = tempdir()?;
        fs::create_dir(root.path().join("existing"))?;
        let workspace = WorkspaceDirectory::open(root.path())?;
        assert!(matches!(
            create_directory(&workspace, "existing"),
            Err(FilesystemCreateDirectoryError::Conflict)
        ));
        assert!(matches!(
            create_directory(&workspace, "missing/child"),
            Err(FilesystemCreateDirectoryError::Path(
                SafePathError::NotFound
            ))
        ));
        assert!(matches!(
            create_directory(&workspace, "../escape"),
            Err(FilesystemCreateDirectoryError::Path(
                SafePathError::InvalidRelativePath
            ))
        ));
        Ok(())
    }
}
