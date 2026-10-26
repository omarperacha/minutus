use super::{
    FilesystemAppliedChange, FilesystemEntry, FilesystemEntryKind, FilesystemFindPathKind,
    FilesystemFindPathQuery, FilesystemFindPathQueryResult, FilesystemFindPathsResult,
    FilesystemListResult, FilesystemMetadata, FilesystemReadTextResult, FilesystemScanMatch,
    FilesystemScanResult, FilesystemSearchMatch, FilesystemSearchResult, FilesystemUndoFileResult,
    FilesystemUndoResult, FilesystemUndoStatus, FilesystemWriteResult, Project, ProjectSummary,
    WorkspaceGrant, WorkspaceGrantSummary,
};

pub(super) fn workspace_summary(grant: &WorkspaceGrant) -> WorkspaceGrantSummary {
    WorkspaceGrantSummary {
        id: grant.id.clone(),
        display_name: grant.display_name.clone(),
        permission_level: grant.permission_level,
        created_at: grant.created_at.clone(),
    }
}

pub(super) fn project_summary(project: &Project) -> ProjectSummary {
    ProjectSummary {
        id: project.id.clone(),
        name: project.name.clone(),
        created_at: project.created_at.clone(),
        updated_at: project.updated_at.clone(),
    }
}

pub(super) fn filesystem_entry_kind(
    kind: filesystem_tools::FilesystemEntryKind,
) -> FilesystemEntryKind {
    match kind {
        filesystem_tools::FilesystemEntryKind::File => FilesystemEntryKind::File,
        filesystem_tools::FilesystemEntryKind::Directory => FilesystemEntryKind::Directory,
    }
}

pub(super) fn filesystem_list_result(
    result: filesystem_tools::FilesystemListResult,
) -> FilesystemListResult {
    FilesystemListResult {
        entries: result
            .entries
            .into_iter()
            .map(|entry| FilesystemEntry {
                name: entry.name,
                relative_path: entry.relative_path,
                kind: filesystem_entry_kind(entry.kind),
                byte_size: entry.byte_size,
            })
            .collect(),
        omitted_count: result.omitted_count,
        truncated: result.truncated,
    }
}

pub(super) fn filesystem_find_paths_result(
    result: filesystem_tools::FilesystemFindPathsResult,
) -> FilesystemFindPathsResult {
    FilesystemFindPathsResult {
        results: result
            .results
            .into_iter()
            .map(|result| FilesystemFindPathQueryResult {
                query: FilesystemFindPathQuery {
                    relative_path: result.query.relative_path,
                    name_contains: result.query.name_contains,
                    extensions: result.query.extensions,
                    kind: match result.query.kind {
                        filesystem_tools::FilesystemFindPathKind::Any => {
                            FilesystemFindPathKind::Any
                        }
                        filesystem_tools::FilesystemFindPathKind::File => {
                            FilesystemFindPathKind::File
                        }
                        filesystem_tools::FilesystemFindPathKind::Directory => {
                            FilesystemFindPathKind::Directory
                        }
                    },
                    max_depth: result.query.max_depth,
                },
                matches: result
                    .matches
                    .into_iter()
                    .map(|entry| FilesystemEntry {
                        name: entry.name,
                        relative_path: entry.relative_path,
                        kind: filesystem_entry_kind(entry.kind),
                        byte_size: entry.byte_size,
                    })
                    .collect(),
                directories_scanned: result.directories_scanned,
                omitted_count: result.omitted_count,
                excluded_count: result.excluded_count,
                truncated: result.truncated,
                complete: result.complete,
            })
            .collect(),
    }
}

pub(super) fn filesystem_metadata(
    metadata: filesystem_tools::FilesystemMetadata,
) -> FilesystemMetadata {
    FilesystemMetadata {
        relative_path: metadata.relative_path,
        kind: filesystem_entry_kind(metadata.kind),
        byte_size: metadata.byte_size,
        modified_unix_ms: metadata.modified_unix_ms,
    }
}

pub(super) fn filesystem_read_text_result(
    result: filesystem_tools::FilesystemReadTextResult,
) -> FilesystemReadTextResult {
    FilesystemReadTextResult {
        relative_path: result.relative_path,
        text: result.text,
        byte_size: result.byte_size,
        content_sha256: result.content_sha256,
    }
}

pub(super) fn filesystem_search_result(
    result: filesystem_tools::FilesystemSearchResult,
) -> FilesystemSearchResult {
    FilesystemSearchResult {
        matches: result
            .matches
            .into_iter()
            .map(|search_match| FilesystemSearchMatch {
                relative_path: search_match.relative_path,
                line_number: search_match.line_number,
                column: search_match.column,
                line: search_match.line,
            })
            .collect(),
        files_searched: result.files_searched,
        omitted_count: result.omitted_count,
        truncated: result.truncated,
    }
}

pub(super) fn filesystem_scan_result(
    result: filesystem_tools::FilesystemScanResult,
) -> FilesystemScanResult {
    FilesystemScanResult {
        matches: result
            .matches
            .into_iter()
            .map(|scan_match| FilesystemScanMatch {
                relative_path: scan_match.relative_path,
                byte_size: scan_match.byte_size,
                line_count: scan_match.line_count,
            })
            .collect(),
        files_scanned: result.files_scanned,
        directories_scanned: result.directories_scanned,
        omitted_count: result.omitted_count,
        excluded_count: result.excluded_count,
        truncated: result.truncated,
    }
}

pub(super) fn filesystem_write_result(
    result: filesystem_tools::FilesystemWriteResult,
) -> FilesystemWriteResult {
    FilesystemWriteResult {
        changes: result
            .changes
            .into_iter()
            .map(|change| FilesystemAppliedChange {
                relative_path: change.relative_path,
                created: change.created,
                previous_sha256: change.previous_sha256,
                previous_content: change.previous_content,
                new_sha256: change.new_sha256,
                verified_byte_size: change.verified_byte_size,
                post_write_verified: change.post_write_verified,
                diff: change.diff,
                created_parent_directories: change.created_parent_directories,
            })
            .collect(),
        postcondition_verified: result.postcondition_verified,
    }
}

pub(super) fn filesystem_undo_result(
    result: filesystem_tools::FilesystemUndoResult,
) -> FilesystemUndoResult {
    FilesystemUndoResult {
        files: result
            .files
            .into_iter()
            .map(|file| FilesystemUndoFileResult {
                relative_path: file.relative_path,
                status: match file.status {
                    filesystem_tools::FilesystemUndoStatus::Undone => FilesystemUndoStatus::Undone,
                    filesystem_tools::FilesystemUndoStatus::Conflict => {
                        FilesystemUndoStatus::Conflict
                    }
                    filesystem_tools::FilesystemUndoStatus::Failed => FilesystemUndoStatus::Failed,
                },
            })
            .collect(),
    }
}
