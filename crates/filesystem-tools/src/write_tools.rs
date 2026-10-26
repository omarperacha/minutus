//! Digest-checked atomic text replacements inside workspace capabilities.

use std::{
    error::Error,
    ffi::OsString,
    fmt::{self, Write as FmtWrite},
    fs::File,
    io::{Read, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
};

use rustix::fs::{self, AtFlags, Mode, OFlags};
use sha2::{Digest, Sha256};

mod staging;

use staging::{complete_diff, io_error, sha256, stage_and_replace, validate_limits};

use crate::{SafePathError, WorkspaceDirectory, safe_path::WriteTarget};

/// Maximum files changed by one call.
pub const MAX_WRITE_FILES: usize = 8;
/// Maximum resulting UTF-8 bytes in one file.
pub const MAX_WRITE_FILE_BYTES: usize = 128 * 1024;
/// Maximum resulting UTF-8 bytes across one call.
pub const MAX_WRITE_TOTAL_BYTES: usize = 192 * 1024;
/// Maximum exact substitutions in one targeted file edit.
pub const MAX_TEXT_REPLACEMENTS: usize = 16;
/// Maximum bytes across old and replacement snippets.
pub const MAX_TEXT_REPLACEMENT_BYTES: usize = 64 * 1024;
/// Maximum required occurrences for one exact substitution.
pub const MAX_TEXT_REPLACEMENT_OCCURRENCES: usize = 32;
const MAX_DIFF_BYTES: usize = 768 * 1024;
const DIFF_CONTEXT_LINES: usize = 3;

/// One complete text replacement requested by the agent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemWriteChange {
    /// Strict path relative to the workspace root.
    pub relative_path: String,
    /// SHA-256 of the exact existing bytes, or `None` only when creating a file.
    pub expected_sha256: Option<String>,
    /// Complete replacement UTF-8 content.
    pub content: String,
    /// Create only the missing ancestors of this exact new-file target.
    pub create_missing_parents: bool,
}

/// One exact bounded substitution performed inside a verified text file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemTextReplacement {
    /// Non-empty exact UTF-8 text that must occur the declared number of times.
    pub old_text: String,
    /// UTF-8 replacement text.
    pub new_text: String,
    /// Exact non-overlapping occurrence count required before replacement.
    pub expected_occurrences: usize,
}

/// One successfully applied text replacement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemAppliedChange {
    /// Validated path relative to the workspace root.
    pub relative_path: String,
    /// Whether the target did not previously exist.
    pub created: bool,
    /// Prior content digest, absent for a newly created file.
    pub previous_sha256: Option<String>,
    /// Exact prior UTF-8 content retained for explicit undo, absent for a new file.
    pub previous_content: Option<String>,
    /// Digest of the applied content.
    pub new_sha256: String,
    /// Exact applied byte length checked after the atomic replacement.
    pub verified_byte_size: u64,
    /// Whether reopening the target reproduced the expected bytes and digest.
    pub post_write_verified: bool,
    /// Complete human-readable unified diff with bounded unchanged context.
    pub diff: String,
    /// Parent directories created solely for this exact file, shallowest first.
    pub created_parent_directories: Vec<String>,
}

/// Successful bounded write result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemWriteResult {
    /// Applied changes in request order.
    pub changes: Vec<FilesystemAppliedChange>,
    /// Whether every requested change and the final aggregate state were verified.
    pub postcondition_verified: bool,
}

/// One prior state used to undo a completed direct write.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemUndoChange {
    /// Validated project-relative path.
    pub relative_path: String,
    /// Digest written by the original operation and required before undo.
    pub applied_sha256: String,
    /// Prior content to restore, or `None` to remove a file created by the write.
    pub previous_content: Option<String>,
    /// Parent directories to remove if they remain empty after undoing a create.
    pub created_parent_directories: Vec<String>,
}

/// Closed per-file result from an explicit undo attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FilesystemUndoStatus {
    /// The prior state was restored.
    Undone,
    /// The current file no longer matches the applied result.
    Conflict,
    /// The path or filesystem operation failed safely.
    Failed,
}

/// One reported file outcome from undo.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemUndoFileResult {
    /// Validated project-relative path from the original write.
    pub relative_path: String,
    /// Sanitized outcome.
    pub status: FilesystemUndoStatus,
}

/// Complete per-file result from an explicit undo attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemUndoResult {
    /// Outcomes in original write order.
    pub files: Vec<FilesystemUndoFileResult>,
}

/// Typed failure from a direct workspace write.
#[derive(Debug)]
pub enum FilesystemWriteError {
    /// The request exceeded a declared size or file-count limit.
    LimitExceeded,
    /// A digest was malformed.
    InvalidDigest,
    /// The expected original state no longer matched.
    Conflict,
    /// A path failed safe resolution.
    Path(SafePathError),
    /// Staging or atomic replacement failed.
    Io(std::io::Error),
    /// Final verification failed and all applied changes were rolled back.
    PostconditionFailed,
}

impl fmt::Display for FilesystemWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LimitExceeded => formatter.write_str("workspace write exceeds a safety limit"),
            Self::InvalidDigest => formatter.write_str("workspace write digest is invalid"),
            Self::Conflict => formatter.write_str("workspace content changed before writing"),
            Self::Path(error) => error.fmt(formatter),
            Self::Io(error) => write!(formatter, "workspace write failed: {error}"),
            Self::PostconditionFailed => {
                formatter.write_str("workspace write postcondition verification failed")
            }
        }
    }
}

impl Error for FilesystemWriteError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Path(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::LimitExceeded
            | Self::InvalidDigest
            | Self::Conflict
            | Self::PostconditionFailed => None,
        }
    }
}

impl From<SafePathError> for FilesystemWriteError {
    fn from(error: SafePathError) -> Self {
        Self::Path(error)
    }
}

struct PreparedChange {
    relative_path: String,
    expected_sha256: Option<String>,
    previous_content: String,
    content: String,
    new_sha256: String,
    diff: String,
    created: bool,
    mode: u32,
}

/// Applies complete UTF-8 replacements using same-directory atomic renames.
///
/// Every target and expected digest is validated before any temporary file is
/// staged. Each target is revalidated immediately before its atomic rename.
///
/// # Errors
///
/// Returns an error for unsafe paths, stale content, malformed digests, limits,
/// unsupported files, or staging/replacement failures.
pub fn write_text(
    workspace: &WorkspaceDirectory,
    changes: &[FilesystemWriteChange],
) -> Result<FilesystemWriteResult, FilesystemWriteError> {
    write_text_with_verifier(workspace, changes, verify_applied)
}

fn write_text_with_verifier<F>(
    workspace: &WorkspaceDirectory,
    changes: &[FilesystemWriteChange],
    verifier: F,
) -> Result<FilesystemWriteResult, FilesystemWriteError>
where
    F: Fn(&WorkspaceDirectory, &PreparedChange) -> bool + Copy,
{
    validate_limits(changes)?;
    let parent_aware = changes
        .iter()
        .filter(|change| change.create_missing_parents)
        .count();
    if parent_aware > 0 {
        if changes.len() != 1 || parent_aware != 1 || changes[0].expected_sha256.is_some() {
            return Err(FilesystemWriteError::Conflict);
        }
        let created = workspace.create_missing_parent_directories_for(&changes[0].relative_path)?;
        let result = write_text_with_existing_parents(workspace, changes, &created, verifier);
        if result.is_err() {
            workspace.remove_empty_directories(&created);
        }
        return result;
    }
    write_text_with_existing_parents(workspace, changes, &[], verifier)
}

fn write_text_with_existing_parents<F>(
    workspace: &WorkspaceDirectory,
    changes: &[FilesystemWriteChange],
    created_parent_directories: &[String],
    verifier: F,
) -> Result<FilesystemWriteResult, FilesystemWriteError>
where
    F: Fn(&WorkspaceDirectory, &PreparedChange) -> bool + Copy,
{
    let mut prepared = Vec::with_capacity(changes.len());
    let mut diff_bytes = 0_usize;
    for change in changes {
        let target = workspace.resolve_write_target(&change.relative_path)?;
        let (previous_content, previous_sha256, mode, created) = if let Some(mut file) =
            target.existing
        {
            let metadata = file.metadata().map_err(FilesystemWriteError::Io)?;
            if metadata.len() > u64::try_from(MAX_WRITE_FILE_BYTES).unwrap_or(u64::MAX) {
                return Err(FilesystemWriteError::LimitExceeded);
            }
            let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
            file.read_to_end(&mut bytes)
                .map_err(FilesystemWriteError::Io)?;
            let content = String::from_utf8(bytes).map_err(|_| FilesystemWriteError::Conflict)?;
            if content.bytes().any(|byte| byte == 0) {
                return Err(FilesystemWriteError::Conflict);
            }
            let digest = sha256(content.as_bytes());
            if change.expected_sha256.as_deref() != Some(digest.as_str()) {
                return Err(FilesystemWriteError::Conflict);
            }
            (content, Some(digest), metadata.permissions().mode(), false)
        } else {
            if change.expected_sha256.is_some() {
                return Err(FilesystemWriteError::Conflict);
            }
            (String::new(), None, 0o644, true)
        };
        let diff = complete_diff(
            &change.relative_path,
            &previous_content,
            &change.content,
            created,
        );
        diff_bytes = diff_bytes.saturating_add(diff.len());
        if diff_bytes > MAX_DIFF_BYTES {
            return Err(FilesystemWriteError::LimitExceeded);
        }
        prepared.push(PreparedChange {
            relative_path: change.relative_path.clone(),
            expected_sha256: previous_sha256,
            previous_content,
            content: change.content.clone(),
            new_sha256: sha256(change.content.as_bytes()),
            diff,
            created,
            mode,
        });
    }

    let mut applied = Vec::with_capacity(prepared.len());
    for change in &prepared {
        if let Err(error) = stage_and_replace(workspace, change) {
            rollback_applied(workspace, &applied);
            return Err(error);
        }
        let post_write_verified = verifier(workspace, change);
        if !post_write_verified {
            let rollback = FilesystemAppliedChange {
                relative_path: change.relative_path.clone(),
                created: change.created,
                previous_sha256: change.expected_sha256.clone(),
                previous_content: (!change.created).then_some(change.previous_content.clone()),
                new_sha256: change.new_sha256.clone(),
                verified_byte_size: u64::try_from(change.content.len()).unwrap_or(u64::MAX),
                post_write_verified: false,
                diff: change.diff.clone(),
                created_parent_directories: created_parent_directories.to_vec(),
            };
            applied.push(rollback);
            rollback_applied(workspace, &applied);
            workspace.remove_empty_directories(created_parent_directories);
            return Err(FilesystemWriteError::PostconditionFailed);
        }
        applied.push(FilesystemAppliedChange {
            relative_path: change.relative_path.clone(),
            created: change.created,
            previous_sha256: change.expected_sha256.clone(),
            previous_content: (!change.created).then_some(change.previous_content.clone()),
            new_sha256: change.new_sha256.clone(),
            verified_byte_size: u64::try_from(change.content.len()).unwrap_or(u64::MAX),
            post_write_verified,
            diff: change.diff.clone(),
            created_parent_directories: if change.created {
                created_parent_directories.to_vec()
            } else {
                Vec::new()
            },
        });
    }
    if !prepared.iter().all(|change| verifier(workspace, change)) {
        rollback_applied(workspace, &applied);
        workspace.remove_empty_directories(created_parent_directories);
        return Err(FilesystemWriteError::PostconditionFailed);
    }
    Ok(FilesystemWriteResult {
        changes: applied,
        postcondition_verified: true,
    })
}

fn rollback_applied(workspace: &WorkspaceDirectory, applied: &[FilesystemAppliedChange]) {
    for change in applied.iter().rev() {
        let undo = FilesystemUndoChange {
            relative_path: change.relative_path.clone(),
            applied_sha256: change.new_sha256.clone(),
            previous_content: change.previous_content.clone(),
            created_parent_directories: change.created_parent_directories.clone(),
        };
        let _ = undo_one(workspace, &undo);
    }
}

fn verify_applied(workspace: &WorkspaceDirectory, change: &PreparedChange) -> bool {
    let Ok(mut file) = workspace.open_file(&change.relative_path) else {
        return false;
    };
    let Ok(metadata) = file.metadata() else {
        return false;
    };
    if metadata.len() != u64::try_from(change.content.len()).unwrap_or(u64::MAX) {
        return false;
    }
    let mut bytes = Vec::with_capacity(change.content.len());
    file.read_to_end(&mut bytes).is_ok()
        && bytes == change.content.as_bytes()
        && sha256(&bytes) == change.new_sha256
}

/// Applies exact substitutions to one existing file without model-side re-emission.
///
/// The current digest and every occurrence count are verified before the
/// resulting complete content enters the atomic direct-write path.
///
/// # Errors
///
/// Returns an error for unsafe paths, malformed limits, stale content,
/// occurrence conflicts, unsupported text, or atomic replacement failures.
pub fn replace_text(
    workspace: &WorkspaceDirectory,
    relative_path: &str,
    expected_sha256: &str,
    replacements: &[FilesystemTextReplacement],
) -> Result<FilesystemWriteResult, FilesystemWriteError> {
    validate_replacement_limits(expected_sha256, replacements)?;
    let mut target = workspace.resolve_write_target(relative_path)?;
    let file = target
        .existing
        .as_mut()
        .ok_or(FilesystemWriteError::Conflict)?;
    let metadata = file.metadata().map_err(FilesystemWriteError::Io)?;
    if metadata.len() > u64::try_from(MAX_WRITE_FILE_BYTES).unwrap_or(u64::MAX) {
        return Err(FilesystemWriteError::LimitExceeded);
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    file.read_to_end(&mut bytes)
        .map_err(FilesystemWriteError::Io)?;
    if sha256(&bytes) != expected_sha256 {
        return Err(FilesystemWriteError::Conflict);
    }
    let mut content = String::from_utf8(bytes).map_err(|_| FilesystemWriteError::Conflict)?;
    if content.bytes().any(|byte| byte == 0) {
        return Err(FilesystemWriteError::Conflict);
    }
    for replacement in replacements {
        if content.matches(&replacement.old_text).count() != replacement.expected_occurrences {
            return Err(FilesystemWriteError::Conflict);
        }
        content = content.replace(&replacement.old_text, &replacement.new_text);
        if content.len() > MAX_WRITE_FILE_BYTES {
            return Err(FilesystemWriteError::LimitExceeded);
        }
    }
    write_text(
        workspace,
        &[FilesystemWriteChange {
            relative_path: relative_path.to_owned(),
            expected_sha256: Some(expected_sha256.to_owned()),
            content,
            create_missing_parents: false,
        }],
    )
}

fn validate_replacement_limits(
    expected_sha256: &str,
    replacements: &[FilesystemTextReplacement],
) -> Result<(), FilesystemWriteError> {
    if expected_sha256.len() != 64
        || !expected_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(FilesystemWriteError::InvalidDigest);
    }
    if replacements.is_empty() || replacements.len() > MAX_TEXT_REPLACEMENTS {
        return Err(FilesystemWriteError::LimitExceeded);
    }
    let mut total = 0_usize;
    for replacement in replacements {
        if replacement.old_text.is_empty()
            || replacement.old_text.contains('\0')
            || replacement.new_text.contains('\0')
            || replacement.expected_occurrences == 0
            || replacement.expected_occurrences > MAX_TEXT_REPLACEMENT_OCCURRENCES
        {
            return Err(FilesystemWriteError::LimitExceeded);
        }
        total = total
            .saturating_add(replacement.old_text.len())
            .saturating_add(replacement.new_text.len());
    }
    if total > MAX_TEXT_REPLACEMENT_BYTES {
        return Err(FilesystemWriteError::LimitExceeded);
    }
    Ok(())
}

/// Attempts to restore every file independently and reports every outcome.
#[must_use]
pub fn undo_text(
    workspace: &WorkspaceDirectory,
    changes: &[FilesystemUndoChange],
) -> FilesystemUndoResult {
    let files = changes
        .iter()
        .map(|change| {
            let status = match undo_one(workspace, change) {
                Ok(()) => FilesystemUndoStatus::Undone,
                Err(FilesystemWriteError::Conflict) => FilesystemUndoStatus::Conflict,
                Err(_) => FilesystemUndoStatus::Failed,
            };
            FilesystemUndoFileResult {
                relative_path: change.relative_path.clone(),
                status,
            }
        })
        .collect();
    FilesystemUndoResult { files }
}

/// Checks whether every file still matches the state required for undo.
#[must_use]
pub fn undo_available(workspace: &WorkspaceDirectory, changes: &[FilesystemUndoChange]) -> bool {
    !changes.is_empty()
        && changes
            .iter()
            .all(|change| applied_content_matches(workspace, change).unwrap_or(false))
}

fn applied_content_matches(
    workspace: &WorkspaceDirectory,
    change: &FilesystemUndoChange,
) -> Result<bool, FilesystemWriteError> {
    let mut target = workspace.resolve_write_target(&change.relative_path)?;
    let Some(file) = target.existing.as_mut() else {
        return Ok(false);
    };
    let metadata = file.metadata().map_err(FilesystemWriteError::Io)?;
    if metadata.len() > u64::try_from(MAX_WRITE_FILE_BYTES).unwrap_or(u64::MAX) {
        return Ok(false);
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    file.read_to_end(&mut bytes)
        .map_err(FilesystemWriteError::Io)?;
    Ok(sha256(&bytes) == change.applied_sha256)
}

fn undo_one(
    workspace: &WorkspaceDirectory,
    change: &FilesystemUndoChange,
) -> Result<(), FilesystemWriteError> {
    if let Some(previous_content) = &change.previous_content {
        write_text(
            workspace,
            &[FilesystemWriteChange {
                relative_path: change.relative_path.clone(),
                expected_sha256: Some(change.applied_sha256.clone()),
                content: previous_content.clone(),
                create_missing_parents: false,
            }],
        )?;
        return Ok(());
    }
    delete_created_file(workspace, change)
}

fn delete_created_file(
    workspace: &WorkspaceDirectory,
    change: &FilesystemUndoChange,
) -> Result<(), FilesystemWriteError> {
    let mut target = workspace.resolve_write_target(&change.relative_path)?;
    let file = target
        .existing
        .as_mut()
        .ok_or(FilesystemWriteError::Conflict)?;
    let metadata = file.metadata().map_err(FilesystemWriteError::Io)?;
    if metadata.len() > u64::try_from(MAX_WRITE_FILE_BYTES).unwrap_or(u64::MAX) {
        return Err(FilesystemWriteError::Conflict);
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    file.read_to_end(&mut bytes)
        .map_err(FilesystemWriteError::Io)?;
    if sha256(&bytes) != change.applied_sha256 {
        return Err(FilesystemWriteError::Conflict);
    }
    let mut current = workspace.resolve_write_target(&change.relative_path)?;
    let current_file = current
        .existing
        .as_mut()
        .ok_or(FilesystemWriteError::Conflict)?;
    let current_metadata = current_file.metadata().map_err(FilesystemWriteError::Io)?;
    let mut current_bytes = Vec::new();
    current_file
        .read_to_end(&mut current_bytes)
        .map_err(FilesystemWriteError::Io)?;
    if metadata.dev() != current_metadata.dev()
        || metadata.ino() != current_metadata.ino()
        || sha256(&current_bytes) != change.applied_sha256
    {
        return Err(FilesystemWriteError::Conflict);
    }
    fs::unlinkat(&current.parent, &current.name, AtFlags::empty()).map_err(io_error)?;
    fs::fsync(&current.parent).map_err(io_error)?;
    workspace.remove_empty_directories(&change.created_parent_directories);
    Ok(())
}

#[cfg(test)]
mod tests;
