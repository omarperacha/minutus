//! Project, conversation, and durable workspace-grant commands.

use super::{
    AppCore, AppStatus, Arc, Conversation, ConversationId, ConversationSummary,
    CreateProjectRequest, DialogExt, IpcError, IpcErrorCode, PathBuf, ProjectId, ProjectSummary,
    ProjectWorkspaceSummary, RenameConversationRequest, State,
    UpdateProjectWorkspacePermissionRequest, ValidateIpc, WorkspaceGrantId, app_core_error,
    current_timestamp, workspace_bookmark,
};

#[tauri::command]
pub(crate) const fn app_status() -> AppStatus {
    AppStatus::ready()
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn list_conversations(
    core: State<'_, Arc<AppCore>>,
    project_id: ProjectId,
) -> Result<Vec<ConversationSummary>, IpcError> {
    project_id.validate()?;
    core.project_conversations(&project_id)
        .map_err(app_core_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn load_conversation(
    core: State<'_, Arc<AppCore>>,
    conversation_id: ConversationId,
) -> Result<Option<Conversation>, IpcError> {
    conversation_id.validate()?;
    core.conversation(&conversation_id).map_err(app_core_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn create_conversation(
    core: State<'_, Arc<AppCore>>,
    project_id: ProjectId,
) -> Result<Conversation, IpcError> {
    project_id.validate()?;
    core.create_project_conversation(&project_id, &current_timestamp()?)
        .map_err(app_core_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn list_projects(
    core: State<'_, Arc<AppCore>>,
) -> Result<Vec<ProjectSummary>, IpcError> {
    core.projects().map_err(app_core_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn create_project(
    core: State<'_, Arc<AppCore>>,
    request: CreateProjectRequest,
) -> Result<ProjectSummary, IpcError> {
    request.validate()?;
    core.create_project(&request.name, &current_timestamp()?)
        .map_err(app_core_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn list_project_workspaces(
    core: State<'_, Arc<AppCore>>,
    project_id: ProjectId,
) -> Result<Vec<ProjectWorkspaceSummary>, IpcError> {
    project_id.validate()?;
    core.project_workspaces(&project_id).map_err(app_core_error)
}

#[tauri::command]
pub(crate) async fn add_project_workspace(
    app: tauri::AppHandle,
    core: State<'_, Arc<AppCore>>,
    project_id: ProjectId,
) -> Result<Option<ProjectWorkspaceSummary>, IpcError> {
    project_id.validate()?;
    let Some((selected_root, bookmark_data)) = pick_durable_workspace(&app)? else {
        return Ok(None);
    };
    let added_at = current_timestamp()?;
    let core = Arc::clone(core.inner());
    tauri::async_runtime::spawn_blocking(move || {
        core.add_project_workspace(&project_id, selected_root, bookmark_data, &added_at)
            .map(Some)
            .map_err(app_core_error)
    })
    .await
    .map_err(|_| IpcError::internal())?
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn remove_project_workspace(
    core: State<'_, Arc<AppCore>>,
    project_id: ProjectId,
    grant_id: WorkspaceGrantId,
) -> Result<bool, IpcError> {
    project_id.validate()?;
    grant_id.validate()?;
    core.remove_project_workspace(&project_id, &grant_id, &current_timestamp()?)
        .map_err(app_core_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn update_project_workspace_permission(
    core: State<'_, Arc<AppCore>>,
    request: UpdateProjectWorkspacePermissionRequest,
) -> Result<ProjectWorkspaceSummary, IpcError> {
    request.validate()?;
    core.update_project_workspace_permission(
        &request.project_id,
        &request.grant_id,
        request.permission_level,
    )
    .map_err(app_core_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn rename_conversation(
    core: State<'_, Arc<AppCore>>,
    request: RenameConversationRequest,
) -> Result<ConversationSummary, IpcError> {
    request.validate()?;
    core.rename_conversation(
        &request.conversation_id,
        request.title.trim(),
        &current_timestamp()?,
    )
    .map_err(app_core_error)?
    .ok_or_else(|| IpcError::invalid_request("The conversation is unavailable"))
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn delete_conversation(
    core: State<'_, Arc<AppCore>>,
    conversation_id: ConversationId,
) -> Result<bool, IpcError> {
    conversation_id.validate()?;
    core.delete_conversation(&conversation_id)
        .map_err(app_core_error)
}

pub(crate) fn pick_durable_workspace(
    app: &tauri::AppHandle,
) -> Result<Option<(PathBuf, Vec<u8>)>, IpcError> {
    let Some(selected) = app.dialog().file().blocking_pick_folder() else {
        return Ok(None);
    };
    let selected_root = selected
        .into_path()
        .map_err(|_| IpcError::invalid_request("The selected workspace is not a local folder"))?;
    let bookmark_data = workspace_bookmark::create(&selected_root).map_err(|()| IpcError {
        code: IpcErrorCode::Unsupported,
        message: "Minutus could not create durable access for the selected folder".to_owned(),
        retryable: true,
    })?;
    Ok(Some((selected_root, bookmark_data)))
}
