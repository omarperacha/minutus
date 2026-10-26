//! Frontend-safe schemas for bounded workspace filesystem tools.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, de::Visitor};
use ts_rs::TS;

use crate::{ConversationId, ProjectId, ToolCallId, WorkspaceGrantId};

/// Request to list one granted workspace directory.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilesystemListRequest {
    /// Project whose settings authorize the grant.
    pub project_id: ProjectId,
    /// Active workspace grant.
    pub grant_id: WorkspaceGrantId,
    /// Strict path relative to the granted root; `.` names the root itself.
    pub relative_path: String,
}

/// Entry-kind filter for recursive filename discovery.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum FilesystemFindPathKind {
    /// Return files and directories.
    Any,
    /// Return regular files only.
    File,
    /// Return directories only.
    Directory,
}

/// One independent recursive filename filter.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilesystemFindPathQuery {
    /// Strict starting directory relative to the granted workspace root.
    pub relative_path: String,
    /// Optional case-insensitive literal fragment matched against basenames.
    /// May be empty only when at least one extension is supplied.
    pub name_contains: String,
    /// Optional case-insensitive extensions without leading dots.
    pub extensions: Vec<String>,
    /// Entry kind to return.
    pub kind: FilesystemFindPathKind,
    /// Maximum depth below the starting directory.
    pub max_depth: usize,
}

/// Request for several bounded recursive filename filters.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilesystemFindPathsRequest {
    /// Project whose settings authorize the grant.
    pub project_id: ProjectId,
    /// Active workspace grant.
    pub grant_id: WorkspaceGrantId,
    /// Independent filters evaluated in request order.
    pub queries: Vec<FilesystemFindPathQuery>,
}

/// Request for safe metadata about one granted workspace entry.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilesystemStatRequest {
    /// Project whose settings authorize the grant.
    pub project_id: ProjectId,
    /// Active workspace grant.
    pub grant_id: WorkspaceGrantId,
    /// Strict path relative to the granted root; `.` names the root itself.
    pub relative_path: String,
}

/// Request to read one bounded UTF-8 workspace file.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilesystemReadTextRequest {
    /// Project whose settings authorize the grant.
    pub project_id: ProjectId,
    /// Active workspace grant.
    pub grant_id: WorkspaceGrantId,
    /// Strict path relative to the granted root.
    pub relative_path: String,
}

/// Request to inspect one image from a granted workspace.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilesystemInspectImageRequest {
    /// Project whose settings authorize the grant.
    pub project_id: ProjectId,
    /// Active workspace grant.
    pub grant_id: WorkspaceGrantId,
    /// Strict path relative to the granted root.
    pub relative_path: String,
}

/// Request for a bounded recursive literal workspace search.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilesystemSearchRequest {
    /// Project whose settings authorize the grant.
    pub project_id: ProjectId,
    /// Active workspace grant.
    pub grant_id: WorkspaceGrantId,
    /// Strict directory path relative to the granted root; `.` names the root.
    pub relative_path: String,
    /// Non-empty case-sensitive literal query.
    pub query: String,
}

/// Request for a bounded aggregate text-file scan.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilesystemScanRequest {
    /// Project whose settings authorize the grant.
    pub project_id: ProjectId,
    /// Active workspace grant.
    pub grant_id: WorkspaceGrantId,
    /// Strict directory path relative to the granted root; `.` names the root.
    pub relative_path: String,
    /// Whether safe descendant directories should be traversed.
    pub recursive: bool,
    /// Minimum line count required for a file to be returned.
    pub minimum_line_count: usize,
    /// Case-insensitive extensions to exclude, without leading dots.
    pub excluded_extensions: Vec<String>,
}

/// One complete UTF-8 replacement in a direct write request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilesystemWriteTextChange {
    /// Strict path relative to the granted root.
    pub relative_path: String,
    /// SHA-256 of existing content, or `None` only to require a new file.
    pub expected_sha256: Option<String>,
    /// Complete replacement UTF-8 content.
    pub content: String,
    /// Create missing ancestors only for this exact new-file target.
    #[serde(default)]
    pub create_missing_parents: bool,
}

/// Request for bounded digest-checked direct text replacements.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilesystemWriteTextRequest {
    /// Project whose settings authorize the grant.
    pub project_id: ProjectId,
    /// Active read-and-write workspace grant.
    pub grant_id: WorkspaceGrantId,
    /// Complete replacements applied in request order after prevalidation.
    pub changes: Vec<FilesystemWriteTextChange>,
}

/// One exact bounded substitution in an existing text file.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilesystemTextReplacement {
    /// Non-empty exact text to replace.
    pub old_text: String,
    /// Replacement text, which may be empty.
    pub new_text: String,
    /// Required number of non-overlapping matches.
    pub expected_occurrences: usize,
}

/// Request for a digest-checked targeted edit without full-file re-emission.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilesystemReplaceTextRequest {
    /// Project whose settings authorize the grant.
    pub project_id: ProjectId,
    /// Active read-and-write workspace grant.
    pub grant_id: WorkspaceGrantId,
    /// Strict existing-file path relative to the granted root.
    pub relative_path: String,
    /// SHA-256 returned by `filesystem.read_text` for the current file.
    pub expected_sha256: String,
    /// Exact substitutions applied in order after validation.
    pub replacements: Vec<FilesystemTextReplacement>,
}

/// Supported safe workspace entry types.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum FilesystemEntryKind {
    /// A regular file.
    File,
    /// A directory on the workspace root's filesystem.
    Directory,
}

/// One safe child returned by `filesystem.list`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilesystemEntry {
    /// Exact UTF-8 file name.
    pub name: String,
    /// Validated path relative to the workspace root.
    pub relative_path: String,
    /// Entry type rechecked from its opened handle.
    pub kind: FilesystemEntryKind,
    /// File length, or `None` for a directory.
    #[ts(type = "number | null")]
    pub byte_size: Option<u64>,
}

/// Bounded result from `filesystem.list`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilesystemListResult {
    /// Safe regular files and directories in deterministic order.
    pub entries: Vec<FilesystemEntry>,
    /// Entries omitted because they were unsafe or unrepresentable.
    pub omitted_count: usize,
    /// Whether scanning stopped at an explicit limit.
    pub truncated: bool,
}

/// Deterministic result for one recursive filename filter.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilesystemFindPathQueryResult {
    /// Exact validated filter, allowing the model to reason about coverage.
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

/// Bounded results for every requested filename filter.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilesystemFindPathsResult {
    /// One result per filter in request order.
    pub results: Vec<FilesystemFindPathQueryResult>,
}

/// Safe metadata returned by `filesystem.stat`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilesystemMetadata {
    /// Validated path relative to the workspace root.
    pub relative_path: String,
    /// Entry type rechecked from its opened handle.
    pub kind: FilesystemEntryKind,
    /// File length, or `None` for a directory.
    #[ts(type = "number | null")]
    pub byte_size: Option<u64>,
    /// Best-effort modification time as Unix milliseconds.
    #[ts(type = "number | null")]
    pub modified_unix_ms: Option<u64>,
}

/// Bounded UTF-8 result from `filesystem.read_text`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilesystemReadTextResult {
    /// Validated path relative to the workspace root.
    pub relative_path: String,
    /// Exact bounded UTF-8 content.
    pub text: String,
    /// Content length in bytes.
    #[ts(type = "number")]
    pub byte_size: u64,
    /// SHA-256 required when replacing this exact content.
    pub content_sha256: String,
}

/// One result from `filesystem.search`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilesystemSearchMatch {
    /// Validated path relative to the workspace root.
    pub relative_path: String,
    /// One-based line number.
    pub line_number: usize,
    /// One-based byte column.
    pub column: usize,
    /// Bounded matching-line preview.
    pub line: String,
}

/// Bounded result from `filesystem.search`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilesystemSearchResult {
    /// Literal matches in deterministic path and line order.
    pub matches: Vec<FilesystemSearchMatch>,
    /// Number of regular UTF-8 files safely inspected.
    pub files_searched: usize,
    /// Files or entries omitted as binary, oversized, unsafe or unrepresentable.
    pub omitted_count: usize,
    /// Whether an explicit result, depth, file or byte limit was reached.
    pub truncated: bool,
}

/// One matching text file returned by `filesystem.scan`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilesystemScanMatch {
    /// Validated path relative to the workspace root.
    pub relative_path: String,
    /// File length in bytes.
    #[ts(type = "number")]
    pub byte_size: u64,
    /// Number of newline-delimited text lines.
    pub line_count: usize,
}

/// Bounded aggregate result from `filesystem.scan`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilesystemScanResult {
    /// Matching file summaries in deterministic path order.
    pub matches: Vec<FilesystemScanMatch>,
    /// Regular UTF-8 text files safely inspected.
    pub files_scanned: usize,
    /// Directories safely inspected.
    pub directories_scanned: usize,
    /// Entries omitted as unsafe, binary, oversized or invalidly encoded.
    pub omitted_count: usize,
    /// Entries skipped by requested or built-in exclusions.
    pub excluded_count: usize,
    /// Whether an explicit resource or result limit was reached.
    pub truncated: bool,
}

/// One atomically applied direct text replacement.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilesystemAppliedChange {
    /// Validated path relative to the workspace root.
    pub relative_path: String,
    /// Whether the operation created a new file.
    pub created: bool,
    /// Prior content digest, absent for a new file.
    pub previous_sha256: Option<String>,
    /// Exact prior content retained for an explicit undo, absent for a new file.
    #[serde(skip_serializing)]
    #[ts(skip)]
    pub previous_content: Option<String>,
    /// Digest of the applied content.
    pub new_sha256: String,
    /// Exact applied byte length checked after the atomic replacement.
    #[ts(type = "number")]
    #[serde(deserialize_with = "deserialize_u64_number")]
    pub verified_byte_size: u64,
    /// Whether reopening the target reproduced the expected bytes and digest.
    pub post_write_verified: bool,
    /// Complete human-readable unified diff with bounded unchanged context.
    pub diff: String,
    /// Parent directories created solely for this file, shallowest first.
    pub created_parent_directories: Vec<String>,
}

/// Successful bounded direct-write result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilesystemWriteResult {
    /// Applied changes in request order.
    pub changes: Vec<FilesystemAppliedChange>,
    /// Verified aggregate postcondition for the complete operation.
    pub postcondition_verified: bool,
}

/// Request to undo the latest persisted direct-write call in a conversation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilesystemUndoRequest {
    /// Project that owns the conversation and workspace.
    pub project_id: ProjectId,
    /// Conversation containing the trusted write call and result.
    pub conversation_id: ConversationId,
    /// Persisted write invocation to undo; it must be the latest write.
    pub write_call_id: ToolCallId,
}

/// Closed per-file outcome from an undo attempt.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum FilesystemUndoStatus {
    /// Prior content was restored or the newly created file was removed.
    Undone,
    /// Current content no longer matched the original applied result.
    Conflict,
    /// The file could not be changed safely.
    Failed,
}

/// One sanitized file outcome from undo.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilesystemUndoFileResult {
    /// Validated project-relative path.
    pub relative_path: String,
    /// Safe outcome without internal path or I/O details.
    pub status: FilesystemUndoStatus,
}

/// Complete persisted result from an explicit undo action.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilesystemUndoResult {
    /// Every original file in write order.
    pub files: Vec<FilesystemUndoFileResult>,
}

/// Native persisted state required to undo one applied file.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilesystemUndoChangeRecord {
    /// Validated project-relative path.
    pub relative_path: String,
    /// Digest written by the original operation.
    pub applied_sha256: String,
    /// Prior content, or absent when the write created the file.
    pub previous_content: Option<String>,
    /// Parent directories removed when empty after undoing a new file.
    #[serde(default)]
    pub created_parent_directories: Vec<String>,
}

fn deserialize_u64_number<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    struct U64NumberVisitor;

    impl Visitor<'_> for U64NumberVisitor {
        type Value = u64;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a non-negative integer")
        }

        fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
            Ok(value)
        }

        fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            u64::try_from(value).map_err(|_| E::custom("integer must be non-negative"))
        }

        fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            if !value.is_finite() || value < 0.0 || value.fract() != 0.0 || value > 131_072.0 {
                return Err(E::custom("number must be a bounded non-negative integer"));
            }
            value
                .to_string()
                .parse()
                .map_err(|_| E::custom("number must be a non-negative integer"))
        }
    }

    deserializer.deserialize_any(U64NumberVisitor)
}
