//! Bounded read-only tools built on workspace directory capabilities.

use std::{
    error::Error,
    ffi::OsStr,
    fmt,
    fs::File,
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    time::{Duration, Instant, UNIX_EPOCH},
};

use rustix::fs::Dir;
use sha2::{Digest, Sha256};

mod scan;

pub use scan::scan;
use scan::{
    child_path, collect_matches, file_changed, io_error, open_supported, read_text_with_limit,
};

use crate::{SafePathError, WorkspaceDirectory};

/// Maximum entries returned by one directory listing.
pub const MAX_LIST_ENTRIES: usize = 512;
/// Maximum UTF-8 bytes returned by one text read.
pub const MAX_READ_TEXT_BYTES: usize = 1024 * 1024;
/// Maximum UTF-8 bytes in one literal search query.
pub const MAX_SEARCH_QUERY_BYTES: usize = 256;
/// Maximum regular files inspected by one search.
pub const MAX_SEARCH_FILES: usize = 512;
/// Maximum directories traversed by one search.
pub const MAX_SEARCH_DIRECTORIES: usize = 512;
/// Maximum bytes read across one search.
pub const MAX_SEARCH_TOTAL_BYTES: usize = 8 * 1024 * 1024;
/// Maximum bytes inspected from any one search candidate.
pub const MAX_SEARCH_FILE_BYTES: usize = MAX_READ_TEXT_BYTES;
/// Maximum matches returned by one search.
pub const MAX_SEARCH_MATCHES: usize = 200;
/// Maximum regular files considered by one aggregate scan.
pub const MAX_SCAN_FILES: usize = 2_048;
/// Maximum directories traversed by one aggregate scan.
pub const MAX_SCAN_DIRECTORIES: usize = 1_024;
/// Maximum bytes inspected across one aggregate scan.
pub const MAX_SCAN_TOTAL_BYTES: usize = 64 * 1024 * 1024;
/// Maximum bytes inspected from one aggregate scan candidate.
pub const MAX_SCAN_FILE_BYTES: usize = 2 * 1024 * 1024;
/// Maximum matching file summaries returned by one aggregate scan.
pub const MAX_SCAN_MATCHES: usize = 500;

const MAX_LIST_SCANNED_ENTRIES: usize = MAX_LIST_ENTRIES * 2;
const MAX_SEARCH_DEPTH: usize = 32;
const MAX_MATCH_LINE_BYTES: usize = 1024;
const MAX_SCAN_DEPTH: usize = 32;
const MAX_SCAN_DURATION: Duration = Duration::from_secs(3);
const DEFAULT_SCAN_EXCLUDED_DIRECTORIES: [&str; 5] =
    [".git", ".cache", "dist", "node_modules", "target"];

/// Supported workspace entry types.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FilesystemEntryKind {
    /// A regular file.
    File,
    /// A directory on the workspace root's filesystem.
    Directory,
}

/// Safe metadata for one opened workspace entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemMetadata {
    /// Validated workspace-relative path.
    pub relative_path: String,
    /// Opened entry type.
    pub kind: FilesystemEntryKind,
    /// File length, or `None` for a directory.
    pub byte_size: Option<u64>,
    /// Best-effort modification time as Unix milliseconds.
    pub modified_unix_ms: Option<u64>,
}

/// One safe child returned by a directory listing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemEntry {
    /// Exact UTF-8 file name.
    pub name: String,
    /// Validated path relative to the workspace root.
    pub relative_path: String,
    /// Opened entry type.
    pub kind: FilesystemEntryKind,
    /// File length, or `None` for a directory.
    pub byte_size: Option<u64>,
}

/// Bounded directory-listing result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemListResult {
    /// Safe regular files and directories, sorted by exact name.
    pub entries: Vec<FilesystemEntry>,
    /// Entries omitted because they were unsafe or could not be represented.
    pub omitted_count: usize,
    /// Whether scanning stopped at an explicit limit.
    pub truncated: bool,
}

/// Bounded UTF-8 file-read result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemReadTextResult {
    /// Validated workspace-relative path.
    pub relative_path: String,
    /// Exact UTF-8 content read from the stable opened handle.
    pub text: String,
    /// Content length in bytes.
    pub byte_size: u64,
    /// SHA-256 of the exact returned UTF-8 bytes.
    pub content_sha256: String,
}

/// Bounded bytes read from one stable workspace file handle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemReadBytesResult {
    /// Validated workspace-relative path.
    pub relative_path: String,
    /// Exact bytes read from the opened file.
    pub bytes: Vec<u8>,
    /// Content length observed on the stable opened handle.
    pub byte_size: u64,
    /// SHA-256 of the exact returned bytes.
    pub content_sha256: String,
}

/// One literal text-search match.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemSearchMatch {
    /// Validated workspace-relative file path.
    pub relative_path: String,
    /// One-based line number.
    pub line_number: usize,
    /// One-based byte column within the line.
    pub column: usize,
    /// Bounded line preview containing the match when possible.
    pub line: String,
}

/// Bounded recursive literal-search result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemSearchResult {
    /// Matches in deterministic path and line order.
    pub matches: Vec<FilesystemSearchMatch>,
    /// Number of regular files safely inspected.
    pub files_searched: usize,
    /// Files or entries omitted as binary, oversized, unsafe or unrepresentable.
    pub omitted_count: usize,
    /// Whether any explicit result, depth, file or byte limit was reached.
    pub truncated: bool,
}

/// One file satisfying an aggregate workspace scan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemScanMatch {
    /// Validated workspace-relative file path.
    pub relative_path: String,
    /// File length rechecked from its stable opened handle.
    pub byte_size: u64,
    /// Number of newline-delimited text lines.
    pub line_count: usize,
}

/// Bounded result from one aggregate workspace scan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemScanResult {
    /// Matching file summaries in deterministic path order.
    pub matches: Vec<FilesystemScanMatch>,
    /// Regular UTF-8 text files safely inspected.
    pub files_scanned: usize,
    /// Directories safely inspected.
    pub directories_scanned: usize,
    /// Entries omitted as unsafe, binary, oversized or invalidly encoded.
    pub omitted_count: usize,
    /// Entries deliberately skipped by extension or built-in directory exclusions.
    pub excluded_count: usize,
    /// Whether an explicit file, directory, depth, byte, result or time limit was reached.
    pub truncated: bool,
}

/// Typed failure from a read-only filesystem tool.
#[derive(Debug)]
pub enum FilesystemToolError {
    /// Safe path resolution rejected the requested entry.
    Path(SafePathError),
    /// A file exceeded the operation's declared byte limit.
    FileTooLarge,
    /// A file was not valid UTF-8.
    InvalidUtf8,
    /// A file contained binary control content.
    BinaryFile,
    /// A search query was empty or exceeded its declared limit.
    InvalidQuery,
    /// Aggregate scan filters were malformed or exceeded their declared limits.
    InvalidScanFilter,
    /// A file changed while it was being read.
    ChangedDuringRead,
    /// Handle-based metadata or I/O failed.
    Io(std::io::Error),
}

impl fmt::Display for FilesystemToolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(error) => error.fmt(formatter),
            Self::FileTooLarge => formatter.write_str("workspace file exceeds the read limit"),
            Self::InvalidUtf8 => formatter.write_str("workspace file is not valid UTF-8 text"),
            Self::BinaryFile => formatter.write_str("workspace file contains binary content"),
            Self::InvalidQuery => formatter.write_str("filesystem search query is invalid"),
            Self::InvalidScanFilter => formatter.write_str("filesystem scan filter is invalid"),
            Self::ChangedDuringRead => {
                formatter.write_str("workspace file changed while it was being read")
            }
            Self::Io(error) => write!(formatter, "workspace read failed: {error}"),
        }
    }
}

impl Error for FilesystemToolError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Path(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::FileTooLarge
            | Self::InvalidUtf8
            | Self::BinaryFile
            | Self::InvalidQuery
            | Self::InvalidScanFilter
            | Self::ChangedDuringRead => None,
        }
    }
}

impl From<SafePathError> for FilesystemToolError {
    fn from(error: SafePathError) -> Self {
        Self::Path(error)
    }
}

/// Lists one directory without following or returning unsafe entries.
///
/// # Errors
///
/// Returns an error when the requested directory cannot be resolved safely or
/// its open handle cannot be read.
pub fn list(
    workspace: &WorkspaceDirectory,
    relative_path: &str,
) -> Result<FilesystemListResult, FilesystemToolError> {
    let directory = workspace.open_directory(relative_path)?;
    let mut stream = Dir::read_from(&directory).map_err(io_error)?;
    let mut entries = Vec::new();
    let mut omitted_count = 0_usize;
    let mut truncated = false;

    for (scanned, child) in (&mut stream).enumerate() {
        if scanned >= MAX_LIST_SCANNED_ENTRIES || entries.len() >= MAX_LIST_ENTRIES {
            truncated = true;
            break;
        }
        let child = child.map_err(io_error)?;
        let name = child.file_name().to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        let Ok(name) = std::str::from_utf8(name) else {
            omitted_count += 1;
            continue;
        };
        let child_path = child_path(relative_path, name);
        match stat(workspace, &child_path) {
            Ok(metadata) => entries.push(FilesystemEntry {
                name: name.to_owned(),
                relative_path: metadata.relative_path,
                kind: metadata.kind,
                byte_size: metadata.byte_size,
            }),
            Err(FilesystemToolError::Path(_)) => omitted_count += 1,
            Err(error) => return Err(error),
        }
    }

    entries.sort_by(|left, right| left.name.as_bytes().cmp(right.name.as_bytes()));
    Ok(FilesystemListResult {
        entries,
        omitted_count,
        truncated,
    })
}

/// Returns metadata rechecked from an opened regular file or directory handle.
///
/// # Errors
///
/// Returns an error when the path is unsafe, an alias or special file, crosses
/// a mounted filesystem, or metadata cannot be read from the opened handle.
pub fn stat(
    workspace: &WorkspaceDirectory,
    relative_path: &str,
) -> Result<FilesystemMetadata, FilesystemToolError> {
    let (file, kind) = open_supported(workspace, relative_path)?;
    let metadata = file.metadata().map_err(FilesystemToolError::Io)?;
    let byte_size = (kind == FilesystemEntryKind::File).then_some(metadata.len());
    let modified_unix_ms = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| u64::try_from(duration.as_millis()).ok());
    Ok(FilesystemMetadata {
        relative_path: relative_path.to_owned(),
        kind,
        byte_size,
        modified_unix_ms,
    })
}

/// Reads one bounded regular UTF-8 text file from its stable opened handle.
///
/// # Errors
///
/// Returns an error for unsafe paths, oversized files, invalid UTF-8, binary
/// content, I/O failures, or a concurrent content change.
pub fn read_text(
    workspace: &WorkspaceDirectory,
    relative_path: &str,
) -> Result<FilesystemReadTextResult, FilesystemToolError> {
    read_text_with_limit(workspace, relative_path, MAX_READ_TEXT_BYTES)
}

/// Reads one bounded regular file from its stable opened handle.
///
/// This broker primitive performs no media decoding. Callers must pass the
/// bytes directly to a bounded format-specific validator before use.
///
/// # Errors
///
/// Returns an error for unsafe paths, oversized files, I/O failures, or a
/// concurrent content change.
pub fn read_bytes(
    workspace: &WorkspaceDirectory,
    relative_path: &str,
    limit: usize,
) -> Result<FilesystemReadBytesResult, FilesystemToolError> {
    if limit == 0 {
        return Err(FilesystemToolError::FileTooLarge);
    }
    let mut file = workspace.open_file(relative_path)?;
    let before = file.metadata().map_err(FilesystemToolError::Io)?;
    if before.len() > u64::try_from(limit).unwrap_or(u64::MAX) {
        return Err(FilesystemToolError::FileTooLarge);
    }
    let capacity = usize::try_from(before.len()).unwrap_or(limit).min(limit);
    let mut bytes = Vec::with_capacity(capacity);
    file.by_ref()
        .take(u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(FilesystemToolError::Io)?;
    if bytes.len() > limit {
        return Err(FilesystemToolError::FileTooLarge);
    }
    let after = file.metadata().map_err(FilesystemToolError::Io)?;
    if file_changed(&before, &after) {
        return Err(FilesystemToolError::ChangedDuringRead);
    }
    let content_sha256 = format!("{:x}", Sha256::digest(&bytes));
    Ok(FilesystemReadBytesResult {
        relative_path: relative_path.to_owned(),
        byte_size: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        content_sha256,
        bytes,
    })
}

/// Recursively searches bounded UTF-8 files for a case-sensitive literal.
///
/// Unsafe, binary and oversized descendants are omitted rather than followed.
///
/// # Errors
///
/// Returns an error when the starting directory is unsafe, the query is invalid
/// or directory access fails before bounded traversal can complete.
pub fn search(
    workspace: &WorkspaceDirectory,
    relative_path: &str,
    query: &str,
) -> Result<FilesystemSearchResult, FilesystemToolError> {
    if query.is_empty() || query.len() > MAX_SEARCH_QUERY_BYTES {
        return Err(FilesystemToolError::InvalidQuery);
    }
    workspace.open_directory(relative_path)?;
    let mut pending = vec![(relative_path.to_owned(), 0_usize)];
    let mut matches = Vec::new();
    let mut files_searched = 0_usize;
    let mut files_considered = 0_usize;
    let mut directories_searched = 0_usize;
    let mut bytes_searched = 0_usize;
    let mut omitted_count = 0_usize;
    let mut truncated = false;

    'search: while let Some((directory, depth)) = pending.pop() {
        if directories_searched >= MAX_SEARCH_DIRECTORIES {
            truncated = true;
            break;
        }
        directories_searched += 1;
        if depth > MAX_SEARCH_DEPTH {
            truncated = true;
            continue;
        }
        let listing = list(workspace, &directory)?;
        omitted_count = omitted_count.saturating_add(listing.omitted_count);
        truncated |= listing.truncated;
        for entry in listing.entries.into_iter().rev() {
            match entry.kind {
                FilesystemEntryKind::Directory => {
                    if depth == MAX_SEARCH_DEPTH || pending.len() >= MAX_SEARCH_DIRECTORIES {
                        truncated = true;
                    } else {
                        pending.push((entry.relative_path, depth + 1));
                    }
                }
                FilesystemEntryKind::File => {
                    if files_considered >= MAX_SEARCH_FILES
                        || bytes_searched >= MAX_SEARCH_TOTAL_BYTES
                        || matches.len() >= MAX_SEARCH_MATCHES
                    {
                        truncated = true;
                        break 'search;
                    }
                    files_considered += 1;
                    let remaining = MAX_SEARCH_TOTAL_BYTES - bytes_searched;
                    let limit = MAX_SEARCH_FILE_BYTES.min(remaining);
                    if entry.byte_size.is_some_and(|byte_size| {
                        byte_size > u64::try_from(limit).unwrap_or(u64::MAX)
                    }) {
                        omitted_count += 1;
                        truncated |= limit < MAX_SEARCH_FILE_BYTES;
                        continue;
                    }
                    match read_text_with_limit(workspace, &entry.relative_path, limit) {
                        Ok(read) => {
                            files_searched += 1;
                            bytes_searched = bytes_searched.saturating_add(read.text.len());
                            collect_matches(&mut matches, &read.relative_path, &read.text, query);
                            if matches.len() >= MAX_SEARCH_MATCHES {
                                truncated = true;
                                break 'search;
                            }
                        }
                        Err(
                            FilesystemToolError::FileTooLarge
                            | FilesystemToolError::InvalidUtf8
                            | FilesystemToolError::BinaryFile
                            | FilesystemToolError::ChangedDuringRead
                            | FilesystemToolError::Path(_),
                        ) => omitted_count += 1,
                        Err(error) => return Err(error),
                    }
                }
            }
        }
    }

    matches.sort_by(|left, right| {
        left.relative_path
            .as_bytes()
            .cmp(right.relative_path.as_bytes())
            .then(left.line_number.cmp(&right.line_number))
            .then(left.column.cmp(&right.column))
    });
    matches.truncate(MAX_SEARCH_MATCHES);
    Ok(FilesystemSearchResult {
        matches,
        files_searched,
        omitted_count,
        truncated,
    })
}

#[cfg(test)]
mod tests;
