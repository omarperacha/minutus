//! Bounded recursive filename discovery without reading file contents.

use std::{
    path::Path,
    time::{Duration, Instant},
};

use crate::{FilesystemEntry, FilesystemEntryKind, FilesystemToolError, WorkspaceDirectory, list};

/// Maximum independent filters accepted by one discovery call.
pub const MAX_FIND_PATH_QUERIES: usize = 8;
/// Maximum matches returned for one filter.
pub const MAX_FIND_PATH_MATCHES: usize = 128;
/// Maximum directories traversed for one filter.
pub const MAX_FIND_PATH_DIRECTORIES: usize = 512;
/// Maximum recursive depth below a filter's starting directory.
pub const MAX_FIND_PATH_DEPTH: usize = 32;
/// Maximum bytes in one case-insensitive filename fragment.
pub const MAX_FIND_PATH_NAME_BYTES: usize = 256;
/// Maximum extensions accepted by one filter.
pub const MAX_FIND_PATH_EXTENSIONS: usize = 16;

const MAX_FIND_DURATION: Duration = Duration::from_secs(2);
const EXCLUDED_DIRECTORIES: [&str; 5] = [".git", ".cache", "dist", "node_modules", "target"];

/// Entry-kind filter for bounded filename discovery.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FilesystemFindPathKind {
    /// Return files and directories.
    Any,
    /// Return regular files only.
    File,
    /// Return directories only.
    Directory,
}

/// One independent bounded filename filter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemFindPathQuery {
    /// Directory relative to the granted workspace root.
    pub relative_path: String,
    /// Case-insensitive literal fragment matched against each basename.
    pub name_contains: String,
    /// Optional case-insensitive file extensions without leading dots.
    pub extensions: Vec<String>,
    /// Entry kind to return.
    pub kind: FilesystemFindPathKind,
    /// Maximum depth below `relative_path`, where immediate children are depth one.
    pub max_depth: usize,
}

/// Deterministic result for one filename filter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemFindPathQueryResult {
    /// Exact validated filter returned for coverage decisions.
    pub query: FilesystemFindPathQuery,
    /// Matching safe entries in deterministic path order.
    pub matches: Vec<FilesystemEntry>,
    /// Directories safely inspected.
    pub directories_scanned: usize,
    /// Unsafe or unrepresentable entries omitted during traversal.
    pub omitted_count: usize,
    /// Directories skipped by built-in exclusions.
    pub excluded_count: usize,
    /// Whether an explicit traversal, result, depth or time bound was reached.
    pub truncated: bool,
    /// Whether the included scope was fully traversed without unsafe omissions.
    pub complete: bool,
}

/// Bounded results for every requested filename filter, in request order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemFindPathsResult {
    /// One result for each requested filter.
    pub results: Vec<FilesystemFindPathQueryResult>,
}

/// Finds safe files and directories by basename without opening file contents.
///
/// # Errors
///
/// Returns an error when filters are malformed or a starting directory cannot
/// be resolved safely.
pub fn find_paths(
    workspace: &WorkspaceDirectory,
    queries: &[FilesystemFindPathQuery],
) -> Result<FilesystemFindPathsResult, FilesystemToolError> {
    validate_queries(queries)?;
    let results = queries
        .iter()
        .map(|query| find_one(workspace, query))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(FilesystemFindPathsResult { results })
}

fn find_one(
    workspace: &WorkspaceDirectory,
    query: &FilesystemFindPathQuery,
) -> Result<FilesystemFindPathQueryResult, FilesystemToolError> {
    workspace.open_directory(&query.relative_path)?;
    let started = Instant::now();
    let name_contains = query.name_contains.to_lowercase();
    let extensions = query
        .extensions
        .iter()
        .map(|extension| extension.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let mut pending = vec![(query.relative_path.clone(), 0_usize)];
    let mut matches = Vec::new();
    let mut directories_scanned = 0_usize;
    let mut omitted_count = 0_usize;
    let mut excluded_count = 0_usize;
    let mut truncated = false;

    'find: while let Some((directory, depth)) = pending.pop() {
        if directories_scanned >= MAX_FIND_PATH_DIRECTORIES
            || started.elapsed() >= MAX_FIND_DURATION
        {
            truncated = true;
            break;
        }
        directories_scanned += 1;
        let listing = list(workspace, &directory)?;
        omitted_count = omitted_count.saturating_add(listing.omitted_count);
        truncated |= listing.truncated;
        for entry in listing.entries.into_iter().rev() {
            if started.elapsed() >= MAX_FIND_DURATION {
                truncated = true;
                break 'find;
            }
            let child_depth = depth.saturating_add(1);
            if entry_matches(&entry, query.kind, &name_contains, &extensions) {
                if matches.len() >= MAX_FIND_PATH_MATCHES {
                    truncated = true;
                    break 'find;
                }
                matches.push(entry.clone());
            }
            if entry.kind == FilesystemEntryKind::Directory {
                if EXCLUDED_DIRECTORIES.contains(&entry.name.as_str()) {
                    excluded_count += 1;
                } else if child_depth < query.max_depth {
                    if pending.len() >= MAX_FIND_PATH_DIRECTORIES {
                        truncated = true;
                    } else {
                        pending.push((entry.relative_path, child_depth));
                    }
                } else if child_depth == query.max_depth {
                    // Reaching the requested boundary is complete, not truncation.
                }
            }
        }
    }
    matches.sort_by(|left, right| {
        left.relative_path
            .as_bytes()
            .cmp(right.relative_path.as_bytes())
    });
    let complete = !truncated && omitted_count == 0;
    Ok(FilesystemFindPathQueryResult {
        query: query.clone(),
        matches,
        directories_scanned,
        omitted_count,
        excluded_count,
        truncated,
        complete,
    })
}

fn entry_matches(
    entry: &FilesystemEntry,
    kind: FilesystemFindPathKind,
    name_contains: &str,
    extensions: &[String],
) -> bool {
    let kind_matches = match kind {
        FilesystemFindPathKind::Any => true,
        FilesystemFindPathKind::File => entry.kind == FilesystemEntryKind::File,
        FilesystemFindPathKind::Directory => entry.kind == FilesystemEntryKind::Directory,
    };
    let extension_matches = extensions.is_empty()
        || Path::new(&entry.name)
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                extensions
                    .iter()
                    .any(|expected| extension.eq_ignore_ascii_case(expected))
            });
    kind_matches && extension_matches && entry.name.to_lowercase().contains(name_contains)
}

fn validate_queries(queries: &[FilesystemFindPathQuery]) -> Result<(), FilesystemToolError> {
    if queries.is_empty() || queries.len() > MAX_FIND_PATH_QUERIES {
        return Err(FilesystemToolError::InvalidQuery);
    }
    for query in queries {
        if query.name_contains.len() > MAX_FIND_PATH_NAME_BYTES
            || (query.name_contains.is_empty() && query.extensions.is_empty())
            || query.name_contains.chars().any(char::is_control)
            || query.max_depth == 0
            || query.max_depth > MAX_FIND_PATH_DEPTH
            || query.extensions.len() > MAX_FIND_PATH_EXTENSIONS
            || query.extensions.iter().any(|extension| {
                extension.is_empty()
                    || extension.len() > 16
                    || !extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
            })
        {
            return Err(FilesystemToolError::InvalidQuery);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{error::Error, fs};

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn batched_discovery_finds_binary_names_and_reports_complete_coverage()
    -> Result<(), Box<dyn Error>> {
        let root = TempDir::new()?;
        fs::create_dir_all(root.path().join("sources/monthly cashflow"))?;
        fs::create_dir_all(root.path().join("outputs/reconstructed_tables"))?;
        fs::write(
            root.path()
                .join("sources/monthly cashflow/monthly cashflow.png"),
            [0_u8, 159, 146, 150],
        )?;
        fs::write(
            root.path()
                .join("outputs/reconstructed_tables/manifest.json"),
            "{}",
        )?;
        let workspace = WorkspaceDirectory::open(root.path())?;
        let result = find_paths(
            &workspace,
            &[
                FilesystemFindPathQuery {
                    relative_path: ".".to_owned(),
                    name_contains: "MONTHLY CASHFLOW".to_owned(),
                    extensions: vec!["png".to_owned()],
                    kind: FilesystemFindPathKind::File,
                    max_depth: 6,
                },
                FilesystemFindPathQuery {
                    relative_path: ".".to_owned(),
                    name_contains: "manifest".to_owned(),
                    extensions: vec!["json".to_owned()],
                    kind: FilesystemFindPathKind::File,
                    max_depth: 6,
                },
            ],
        )?;

        assert_eq!(result.results.len(), 2);
        assert_eq!(
            result.results[0].matches[0].relative_path,
            "sources/monthly cashflow/monthly cashflow.png"
        );
        assert_eq!(
            result.results[1].matches[0].relative_path,
            "outputs/reconstructed_tables/manifest.json"
        );
        assert!(result.results.iter().all(|result| !result.truncated));
        assert!(result.results.iter().all(|result| result.complete));
        Ok(())
    }

    #[test]
    fn discovery_is_depth_and_filter_bounded() -> Result<(), Box<dyn Error>> {
        let root = TempDir::new()?;
        fs::create_dir_all(root.path().join("one/two"))?;
        fs::write(root.path().join("one/report.csv"), "one")?;
        fs::write(root.path().join("one/two/report.csv"), "two")?;
        fs::write(root.path().join("one/report.png"), "binary")?;
        let workspace = WorkspaceDirectory::open(root.path())?;
        let result = find_paths(
            &workspace,
            &[FilesystemFindPathQuery {
                relative_path: "one".to_owned(),
                name_contains: "report".to_owned(),
                extensions: vec!["csv".to_owned()],
                kind: FilesystemFindPathKind::File,
                max_depth: 1,
            }],
        )?;

        assert_eq!(result.results[0].matches.len(), 1);
        assert_eq!(result.results[0].matches[0].relative_path, "one/report.csv");
        assert!(!result.results[0].truncated);
        assert!(result.results[0].complete);
        Ok(())
    }
}
