//! Adapter from validated chat calls to the filesystem capability broker.

use std::collections::BTreeMap;

use app_core::AppCore;
use ipc_types::{
    AssetId, FilesystemUndoChangeRecord, FilesystemWriteResult, IpcError, JsonValue, ProjectId,
};
use model_runtime::ParsedToolCall;

use crate::{
    error_mapping::{app_core_error, bookmark_access_error, current_timestamp},
    image_commands::image_import_error,
    workspace_bookmark,
};

use super::{
    ChatToolWorkspace, ExecutedChatTool, collect_evidence,
    validation::{
        required_bool_argument, required_find_path_queries_argument, required_string_argument,
        required_string_array_argument, required_text_replacements_argument,
        required_usize_argument, required_write_changes_argument,
    },
};

pub(super) fn run_chat_filesystem_tool(
    core: &AppCore,
    project_id: &ProjectId,
    workspace: &ChatToolWorkspace,
    call: &ParsedToolCall,
) -> Result<ExecutedChatTool, IpcError> {
    let bookmark_data = core
        .project_workspace_bookmark_data(project_id, &workspace.grant_id)
        .map_err(app_core_error)?;
    let access = workspace_bookmark::access(&bookmark_data).map_err(bookmark_access_error)?;
    run_chat_filesystem_tool_at_root(core, workspace, call, access.path())
}

fn run_chat_filesystem_tool_at_root(
    core: &AppCore,
    workspace: &ChatToolWorkspace,
    call: &ParsedToolCall,
    resolved_root: &std::path::Path,
) -> Result<ExecutedChatTool, IpcError> {
    let mut undo_changes = Vec::new();
    let mut inspected_asset_id = None;
    let value = match call.name.as_str() {
        "filesystem.list" => serde_json::to_value(
            core.filesystem_list(
                &workspace.grant_id,
                resolved_root,
                required_string_argument(&call.arguments, "path")?,
            )
            .map_err(app_core_error)?,
        ),
        "filesystem.find_paths" => serde_json::to_value(
            core.filesystem_find_paths(
                &workspace.grant_id,
                resolved_root,
                &required_find_path_queries_argument(&call.arguments)?,
            )
            .map_err(app_core_error)?,
        ),
        "filesystem.collect_evidence" => {
            let (result, asset_id) =
                collect_evidence::execute(core, workspace, resolved_root, &call.arguments)?;
            inspected_asset_id = asset_id;
            Ok(result)
        }
        "filesystem.stat" => serde_json::to_value(
            core.filesystem_stat(
                &workspace.grant_id,
                resolved_root,
                required_string_argument(&call.arguments, "path")?,
            )
            .map_err(app_core_error)?,
        ),
        "filesystem.read_text" => serde_json::to_value(
            core.filesystem_read_text(
                &workspace.grant_id,
                resolved_root,
                required_string_argument(&call.arguments, "path")?,
            )
            .map_err(app_core_error)?,
        ),
        "filesystem.inspect_image" => {
            let (asset_id, metadata) = inspect_chat_image(core, workspace, call, resolved_root)?;
            inspected_asset_id = Some(asset_id);
            serde_json::to_value(metadata)
        }
        "filesystem.search" => serde_json::to_value(
            core.filesystem_search(
                &workspace.grant_id,
                resolved_root,
                required_string_argument(&call.arguments, "path")?,
                required_string_argument(&call.arguments, "query")?,
            )
            .map_err(app_core_error)?,
        ),
        "filesystem.scan" => serde_json::to_value(
            core.filesystem_scan(
                &workspace.grant_id,
                resolved_root,
                required_string_argument(&call.arguments, "path")?,
                required_bool_argument(&call.arguments, "recursive")?,
                required_usize_argument(&call.arguments, "minimumLineCount")?,
                &required_string_array_argument(&call.arguments, "excludeExtensions")?,
            )
            .map_err(app_core_error)?,
        ),
        "filesystem.create_directory" => {
            Ok(create_chat_directory(core, workspace, call, resolved_root)?)
        }
        "filesystem.write_text" => {
            let result = write_chat_text(core, workspace, call, resolved_root)?;
            undo_changes = filesystem_undo_changes(&result);
            serde_json::to_value(result)
        }
        "filesystem.replace_text" => {
            let result = core
                .filesystem_replace_text(
                    &workspace.grant_id,
                    resolved_root,
                    required_string_argument(&call.arguments, "path")?,
                    required_string_argument(&call.arguments, "expectedSha256")?,
                    &required_text_replacements_argument(&call.arguments)?,
                )
                .map_err(app_core_error)?;
            undo_changes = filesystem_undo_changes(&result);
            serde_json::to_value(result)
        }
        _ => {
            return Err(IpcError::invalid_request(
                "The model requested an unavailable tool",
            ));
        }
    }
    .map_err(|_| IpcError::internal())?;
    Ok(ExecutedChatTool {
        value: ipc_json_value(&value)?,
        undo_changes,
        inspected_asset_id,
    })
}

fn create_chat_directory(
    core: &AppCore,
    workspace: &ChatToolWorkspace,
    call: &ParsedToolCall,
    resolved_root: &std::path::Path,
) -> Result<serde_json::Value, IpcError> {
    let result = core
        .filesystem_create_directory(
            &workspace.grant_id,
            resolved_root,
            required_string_argument(&call.arguments, "path")?,
        )
        .map_err(app_core_error)?;
    Ok(serde_json::json!({
        "created": result.created,
        "path": result.relative_path,
        "postCreateVerified": result.post_create_verified
    }))
}

fn write_chat_text(
    core: &AppCore,
    workspace: &ChatToolWorkspace,
    call: &ParsedToolCall,
    resolved_root: &std::path::Path,
) -> Result<FilesystemWriteResult, IpcError> {
    core.filesystem_write_text(
        &workspace.grant_id,
        resolved_root,
        &required_write_changes_argument(&call.arguments)?,
    )
    .map_err(app_core_error)
}

fn inspect_chat_image(
    core: &AppCore,
    workspace: &ChatToolWorkspace,
    call: &ParsedToolCall,
    resolved_root: &std::path::Path,
) -> Result<(AssetId, serde_json::Value), IpcError> {
    inspect_chat_image_path(
        core,
        workspace,
        required_string_argument(&call.arguments, "path")?,
        resolved_root,
    )
}

pub(super) fn inspect_chat_image_path(
    core: &AppCore,
    workspace: &ChatToolWorkspace,
    relative_path: &str,
    resolved_root: &std::path::Path,
) -> Result<(AssetId, serde_json::Value), IpcError> {
    let asset = core
        .filesystem_inspect_image(
            &workspace.grant_id,
            resolved_root,
            relative_path,
            &current_timestamp()?,
        )
        .map_err(|error| match error {
            error @ app_core::AppCoreError::AssetStore(_) => image_import_error(error),
            other => app_core_error(other),
        })?;
    let metadata = serde_json::json!({
        "status": "image_ready",
        "width": asset.derivative.width,
        "height": asset.derivative.height,
    });
    Ok((asset.id, metadata))
}

pub(super) fn filesystem_undo_changes(
    result: &FilesystemWriteResult,
) -> Vec<FilesystemUndoChangeRecord> {
    result
        .changes
        .iter()
        .map(|change| FilesystemUndoChangeRecord {
            relative_path: change.relative_path.clone(),
            applied_sha256: change.new_sha256.clone(),
            previous_content: change.previous_content.clone(),
            created_parent_directories: change.created_parent_directories.clone(),
        })
        .collect()
}

pub(crate) fn ipc_json_value(value: &serde_json::Value) -> Result<JsonValue, IpcError> {
    match value {
        serde_json::Value::Null => Ok(JsonValue::Null),
        serde_json::Value::Bool(value) => Ok(JsonValue::Bool(*value)),
        serde_json::Value::Number(value) => value
            .as_f64()
            .filter(|value| value.is_finite())
            .map(JsonValue::Number)
            .ok_or_else(IpcError::internal),
        serde_json::Value::String(value) => Ok(JsonValue::String(value.clone())),
        serde_json::Value::Array(values) => values
            .iter()
            .map(ipc_json_value)
            .collect::<Result<Vec<_>, _>>()
            .map(JsonValue::Array),
        serde_json::Value::Object(values) => values
            .iter()
            .map(|(name, value)| Ok((name.clone(), ipc_json_value(value)?)))
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map(JsonValue::Object),
    }
}
