//! Project-scoped filesystem IPC commands and undo planning.

use super::{
    AppCore, AppCoreError, Arc, Conversation, FilesystemListRequest, FilesystemListResult,
    FilesystemMetadata, FilesystemReadTextRequest, FilesystemReadTextResult,
    FilesystemReplaceTextRequest, FilesystemScanRequest, FilesystemScanResult,
    FilesystemSearchRequest, FilesystemSearchResult, FilesystemStatRequest, FilesystemUndoChange,
    FilesystemUndoRequest, FilesystemWriteResult, FilesystemWriteTextRequest, IpcError,
    MessagePart, Path, ProjectId, State, ToolCallId, ToolOutcome, ToolResultPart, ValidateIpc,
    WorkspaceGrantId, app_core_error, bookmark_access_error, current_timestamp, workspace_bookmark,
};

#[tauri::command]
pub(crate) async fn filesystem_list(
    core: State<'_, Arc<AppCore>>,
    request: FilesystemListRequest,
) -> Result<FilesystemListResult, IpcError> {
    request.validate()?;
    let project_id = request.project_id;
    let grant_id = request.grant_id;
    let relative_path = request.relative_path;
    run_workspace_operation(
        Arc::clone(core.inner()),
        project_id,
        grant_id.clone(),
        move |core, root| core.filesystem_list(&grant_id, root, &relative_path),
    )
    .await
}

#[tauri::command]
pub(crate) async fn filesystem_stat(
    core: State<'_, Arc<AppCore>>,
    request: FilesystemStatRequest,
) -> Result<FilesystemMetadata, IpcError> {
    request.validate()?;
    let project_id = request.project_id;
    let grant_id = request.grant_id;
    let relative_path = request.relative_path;
    run_workspace_operation(
        Arc::clone(core.inner()),
        project_id,
        grant_id.clone(),
        move |core, root| core.filesystem_stat(&grant_id, root, &relative_path),
    )
    .await
}

#[tauri::command]
pub(crate) async fn filesystem_read_text(
    core: State<'_, Arc<AppCore>>,
    request: FilesystemReadTextRequest,
) -> Result<FilesystemReadTextResult, IpcError> {
    request.validate()?;
    let project_id = request.project_id;
    let grant_id = request.grant_id;
    let relative_path = request.relative_path;
    run_workspace_operation(
        Arc::clone(core.inner()),
        project_id,
        grant_id.clone(),
        move |core, root| core.filesystem_read_text(&grant_id, root, &relative_path),
    )
    .await
}

#[tauri::command]
pub(crate) async fn filesystem_write_text(
    core: State<'_, Arc<AppCore>>,
    request: FilesystemWriteTextRequest,
) -> Result<FilesystemWriteResult, IpcError> {
    request.validate()?;
    let project_id = request.project_id;
    let grant_id = request.grant_id;
    let changes = request.changes;
    run_workspace_operation(
        Arc::clone(core.inner()),
        project_id,
        grant_id.clone(),
        move |core, root| core.filesystem_write_text(&grant_id, root, &changes),
    )
    .await
}

#[tauri::command]
pub(crate) async fn filesystem_replace_text(
    core: State<'_, Arc<AppCore>>,
    request: FilesystemReplaceTextRequest,
) -> Result<FilesystemWriteResult, IpcError> {
    request.validate()?;
    let project_id = request.project_id;
    let grant_id = request.grant_id;
    let relative_path = request.relative_path;
    let expected_sha256 = request.expected_sha256;
    let replacements = request.replacements;
    run_workspace_operation(
        Arc::clone(core.inner()),
        project_id,
        grant_id.clone(),
        move |core, root| {
            core.filesystem_replace_text(
                &grant_id,
                root,
                &relative_path,
                &expected_sha256,
                &replacements,
            )
        },
    )
    .await
}

#[tauri::command]
pub(crate) async fn undo_filesystem_write(
    core: State<'_, Arc<AppCore>>,
    request: FilesystemUndoRequest,
) -> Result<Conversation, IpcError> {
    request.validate()?;
    let mut conversation = core
        .conversation(&request.conversation_id)
        .map_err(app_core_error)?
        .ok_or_else(|| IpcError::invalid_request("The conversation is unavailable"))?;
    if conversation.project_id != request.project_id {
        return Err(IpcError::invalid_request(
            "The conversation does not belong to this project",
        ));
    }
    let (grant_id, changes, result_message_index) =
        filesystem_undo_plan(&conversation, &request.write_call_id)?;
    let undo_result = run_workspace_operation(
        Arc::clone(core.inner()),
        request.project_id,
        grant_id.clone(),
        move |core, root| core.filesystem_undo_text(&grant_id, root, &changes),
    )
    .await?;
    conversation.messages[result_message_index]
        .parts
        .push(MessagePart::FilesystemUndo {
            write_call_id: request.write_call_id,
            result: undo_result,
        });
    conversation.updated_at = current_timestamp()?;
    core.save_conversation(&conversation)
        .map_err(app_core_error)?;
    Ok(conversation)
}

#[tauri::command]
pub(crate) async fn filesystem_undo_available(
    core: State<'_, Arc<AppCore>>,
    request: FilesystemUndoRequest,
) -> Result<bool, IpcError> {
    request.validate()?;
    let conversation = core
        .conversation(&request.conversation_id)
        .map_err(app_core_error)?
        .ok_or_else(|| IpcError::invalid_request("The conversation is unavailable"))?;
    if conversation.project_id != request.project_id {
        return Err(IpcError::invalid_request(
            "The conversation does not belong to this project",
        ));
    }
    let (grant_id, changes, _) = filesystem_undo_plan(&conversation, &request.write_call_id)?;
    run_workspace_operation(
        Arc::clone(core.inner()),
        request.project_id,
        grant_id.clone(),
        move |core, root| core.filesystem_undo_available(&grant_id, root, &changes),
    )
    .await
}

pub(crate) fn filesystem_undo_plan(
    conversation: &Conversation,
    write_call_id: &ToolCallId,
) -> Result<(WorkspaceGrantId, Vec<FilesystemUndoChange>, usize), IpcError> {
    let latest_write_call = conversation
        .messages
        .iter()
        .flat_map(|message| &message.parts)
        .filter_map(|part| match part {
            MessagePart::FilesystemWriteRecord { write_call_id, .. } => Some(write_call_id),
            _ => None,
        })
        .next_back()
        .ok_or_else(|| IpcError::invalid_request("No filesystem write is available to undo"))?;
    if latest_write_call != write_call_id {
        return Err(IpcError::invalid_request(
            "Only the latest filesystem write can be undone",
        ));
    }

    let mut grant_id = None;
    let mut undo_changes = None;
    let mut write_result = None;
    let mut result_message_index = None;
    for (message_index, message) in conversation.messages.iter().enumerate() {
        for part in &message.parts {
            match part {
                MessagePart::FilesystemWriteRecord {
                    write_call_id: recorded_call_id,
                    grant_id: recorded_grant_id,
                    changes,
                } if recorded_call_id == write_call_id => {
                    grant_id = Some(recorded_grant_id.clone());
                    undo_changes = Some(changes.clone());
                    result_message_index = Some(message_index);
                }
                MessagePart::FilesystemUndo {
                    write_call_id: undone_call_id,
                    ..
                } if undone_call_id == write_call_id => {
                    return Err(IpcError::invalid_request(
                        "This filesystem write has already been undone",
                    ));
                }
                MessagePart::ToolResult { result } if result.call_id == *write_call_id => {
                    if let ToolOutcome::Completed { content } = &result.outcome {
                        write_result = content.iter().find_map(|part| match part {
                            ToolResultPart::Json { value } => serde_json::to_value(value)
                                .ok()
                                .and_then(|value| serde_json::from_value(value).ok()),
                            ToolResultPart::Text { .. } | ToolResultPart::Asset { .. } => None,
                        });
                    }
                }
                _ => {}
            }
        }
    }
    let write_result: FilesystemWriteResult = write_result
        .ok_or_else(|| IpcError::invalid_request("The write history cannot be undone safely"))?;
    let records = undo_changes
        .ok_or_else(|| IpcError::invalid_request("The write history cannot be undone safely"))?;
    if records.is_empty()
        || records.len() != write_result.changes.len()
        || records
            .iter()
            .zip(&write_result.changes)
            .any(|(record, applied)| {
                record.relative_path != applied.relative_path
                    || record.applied_sha256 != applied.new_sha256
            })
    {
        return Err(IpcError::invalid_request(
            "The write history cannot be undone safely",
        ));
    }
    let changes = records
        .into_iter()
        .map(|change| FilesystemUndoChange {
            relative_path: change.relative_path,
            applied_sha256: change.applied_sha256,
            previous_content: change.previous_content,
            created_parent_directories: change.created_parent_directories,
        })
        .collect::<Vec<_>>();
    Ok((
        grant_id.ok_or_else(|| {
            IpcError::invalid_request("The original workspace grant is unavailable")
        })?,
        changes,
        result_message_index.ok_or_else(IpcError::internal)?,
    ))
}

#[tauri::command]
pub(crate) async fn filesystem_search(
    core: State<'_, Arc<AppCore>>,
    request: FilesystemSearchRequest,
) -> Result<FilesystemSearchResult, IpcError> {
    request.validate()?;
    let project_id = request.project_id;
    let grant_id = request.grant_id;
    let relative_path = request.relative_path;
    let query = request.query;
    run_workspace_operation(
        Arc::clone(core.inner()),
        project_id,
        grant_id.clone(),
        move |core, root| core.filesystem_search(&grant_id, root, &relative_path, &query),
    )
    .await
}

#[tauri::command]
pub(crate) async fn filesystem_scan(
    core: State<'_, Arc<AppCore>>,
    request: FilesystemScanRequest,
) -> Result<FilesystemScanResult, IpcError> {
    request.validate()?;
    let project_id = request.project_id;
    let grant_id = request.grant_id;
    let relative_path = request.relative_path;
    let recursive = request.recursive;
    let minimum_line_count = request.minimum_line_count;
    let excluded_extensions = request.excluded_extensions;
    run_workspace_operation(
        Arc::clone(core.inner()),
        project_id,
        grant_id.clone(),
        move |core, root| {
            core.filesystem_scan(
                &grant_id,
                root,
                &relative_path,
                recursive,
                minimum_line_count,
                &excluded_extensions,
            )
        },
    )
    .await
}

pub(crate) async fn run_workspace_operation<T, F>(
    core: Arc<AppCore>,
    project_id: ProjectId,
    grant_id: WorkspaceGrantId,
    operation: F,
) -> Result<T, IpcError>
where
    T: Send + 'static,
    F: FnOnce(&AppCore, &Path) -> Result<T, AppCoreError> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(move || {
        let bookmark_data = core
            .project_workspace_bookmark_data(&project_id, &grant_id)
            .map_err(app_core_error)?;
        let workspace =
            workspace_bookmark::access(&bookmark_data).map_err(bookmark_access_error)?;
        operation(&core, workspace.path()).map_err(app_core_error)
    })
    .await
    .map_err(|_| IpcError::internal())?
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use ipc_types::{
        ConversationId, FilesystemUndoChangeRecord, Message, MessageId, MessageRole, ProjectId,
        ToolCallId, ToolName, ToolOutcome, ToolRequest, ToolResult, ToolResultPart,
        WorkspaceGrantId,
    };

    use crate::chat_tools::ipc_json_value;

    use super::*;

    #[test]
    fn filesystem_undo_plan_is_bound_to_latest_recorded_write() -> Result<(), IpcError> {
        let call_id = ToolCallId("write-1".to_owned());
        let grant_id = WorkspaceGrantId("grant-1".to_owned());
        let write_result = FilesystemWriteResult {
            changes: vec![ipc_types::FilesystemAppliedChange {
                relative_path: "src/lib.rs".to_owned(),
                created: false,
                previous_sha256: Some("a".repeat(64)),
                previous_content: Some("before\n".to_owned()),
                new_sha256: "b".repeat(64),
                verified_byte_size: 6,
                post_write_verified: true,
                diff: "--- src/lib.rs\n+++ src/lib.rs\n".to_owned(),
                created_parent_directories: Vec::new(),
            }],
            postcondition_verified: true,
        };
        let mut conversation = Conversation {
            id: ConversationId("conversation-1".to_owned()),
            project_id: ProjectId("project-1".to_owned()),
            title: "Write".to_owned(),
            messages: vec![
                Message {
                    id: MessageId("message-1".to_owned()),
                    conversation_id: ConversationId("conversation-1".to_owned()),
                    role: MessageRole::Assistant,
                    parts: vec![MessagePart::ToolCall {
                        request: ToolRequest {
                            call_id: call_id.clone(),
                            name: ToolName::FilesystemWriteText,
                            arguments: BTreeMap::new(),
                        },
                    }],
                    created_at: "2026-08-16T12:00:00Z".to_owned(),
                },
                Message {
                    id: MessageId("message-2".to_owned()),
                    conversation_id: ConversationId("conversation-1".to_owned()),
                    role: MessageRole::Tool,
                    parts: vec![
                        MessagePart::ToolResult {
                            result: ToolResult {
                                call_id: call_id.clone(),
                                outcome: ToolOutcome::Completed {
                                    content: vec![ToolResultPart::Json {
                                        value: ipc_json_value(
                                            &serde_json::to_value(write_result)
                                                .map_err(|_| IpcError::internal())?,
                                        )?,
                                    }],
                                },
                            },
                        },
                        MessagePart::FilesystemWriteRecord {
                            write_call_id: call_id.clone(),
                            grant_id: grant_id.clone(),
                            changes: vec![FilesystemUndoChangeRecord {
                                relative_path: "src/lib.rs".to_owned(),
                                applied_sha256: "b".repeat(64),
                                previous_content: Some("before\n".to_owned()),
                                created_parent_directories: Vec::new(),
                            }],
                        },
                    ],
                    created_at: "2026-08-16T12:00:00Z".to_owned(),
                },
            ],
            created_at: "2026-08-16T12:00:00Z".to_owned(),
            updated_at: "2026-08-16T12:00:00Z".to_owned(),
        };
        let (planned_grant, changes, message_index) =
            filesystem_undo_plan(&conversation, &call_id)?;
        assert_eq!(planned_grant, grant_id);
        assert_eq!(changes[0].previous_content.as_deref(), Some("before\n"));
        assert_eq!(message_index, 1);

        let persisted = serde_json::to_string(&conversation).map_err(|_| IpcError::internal())?;
        let reloaded: Conversation =
            serde_json::from_str(&persisted).map_err(|_| IpcError::internal())?;
        let (persisted_grant, persisted_changes, persisted_message_index) =
            filesystem_undo_plan(&reloaded, &call_id)?;
        assert_eq!(persisted_grant, grant_id);
        assert_eq!(
            persisted_changes[0].previous_content.as_deref(),
            Some("before\n")
        );
        assert_eq!(persisted_message_index, 1);

        conversation.messages[1]
            .parts
            .push(MessagePart::FilesystemUndo {
                write_call_id: call_id.clone(),
                result: ipc_types::FilesystemUndoResult { files: Vec::new() },
            });
        assert!(filesystem_undo_plan(&conversation, &call_id).is_err());
        Ok(())
    }
}
