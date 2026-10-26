//! Tauri commands that start and stop chat generation.

use std::{sync::Arc, thread};

use app_core::AppCore;
use ipc_types::{ChatGenerationAccepted, ChatRequest, IpcError, RequestId, ValidateIpc};
use tauri::State;

use crate::{
    app_state::{ChatGenerationControl, InferenceExecutable},
    error_mapping::{app_core_error, current_timestamp},
};

use super::generation::{ChatGenerationSettings, run_chat_generation};

#[tauri::command]
pub(crate) async fn start_chat_generation(
    app: tauri::AppHandle,
    core: State<'_, Arc<AppCore>>,
    control: State<'_, Arc<ChatGenerationControl>>,
    executable: State<'_, InferenceExecutable>,
    request: ChatRequest,
) -> Result<ChatGenerationAccepted, IpcError> {
    let core = Arc::clone(core.inner());
    let control = Arc::clone(control.inner());
    let executable = executable.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        start_chat_generation_blocking(app, &core, &control, executable, request)
    })
    .await
    .map_err(|_| IpcError::internal())?
}

fn start_chat_generation_blocking(
    app: tauri::AppHandle,
    core: &Arc<AppCore>,
    control: &Arc<ChatGenerationControl>,
    executable: std::path::PathBuf,
    request: ChatRequest,
) -> Result<ChatGenerationAccepted, IpcError> {
    request.validate()?;
    let started_at = current_timestamp()?;
    let request_id = RequestId(uuid::Uuid::now_v7().to_string());
    let activity = core.begin_chat_activity().map_err(app_core_error)?;
    let cancellation_requested = control.begin(request_id.clone(), activity)?;
    let project_id = request.project_id;
    let conversation_id = match request.conversation_id {
        Some(conversation_id) => {
            let owned = match core.conversation(&conversation_id) {
                Ok(conversation) => {
                    conversation.is_some_and(|conversation| conversation.project_id == project_id)
                }
                Err(error) => {
                    control.finish(&request_id);
                    return Err(app_core_error(error));
                }
            };
            if !owned {
                control.finish(&request_id);
                return Err(IpcError::invalid_request(
                    "The conversation does not belong to the selected project",
                ));
            }
            conversation_id
        }
        None => match core.create_project_conversation(&project_id, &started_at) {
            Ok(conversation) => conversation.id,
            Err(error) => {
                control.finish(&request_id);
                return Err(app_core_error(error));
            }
        },
    };
    if let Some(selection) = &request.model_selection
        && let Err(error) = core.set_conversation_model(&conversation_id, selection)
    {
        control.finish(&request_id);
        return Err(app_core_error(error));
    }
    let settings = ChatGenerationSettings {
        context_tokens: request.context_tokens,
        effort: request.effort,
    };
    let conversation = match core.append_user_message(
        &conversation_id,
        request.text,
        request.asset_ids,
        &started_at,
    ) {
        Ok(conversation) => conversation,
        Err(error) => {
            control.finish(&request_id);
            return Err(app_core_error(error));
        }
    };
    let accepted = ChatGenerationAccepted {
        request_id: request_id.clone(),
        conversation: conversation.clone(),
    };
    let worker_core = Arc::clone(core);
    let worker_control = Arc::clone(control);
    thread::Builder::new()
        .name("zeta-chat-generation".to_owned())
        .spawn(move || {
            run_chat_generation(
                app,
                worker_core,
                worker_control,
                executable,
                request_id,
                conversation,
                cancellation_requested,
                settings,
            );
        })
        .map_err(|_| {
            control.finish(&accepted.request_id);
            IpcError::internal()
        })?;
    Ok(accepted)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn stop_chat_generation(
    core: State<'_, Arc<AppCore>>,
    control: State<'_, Arc<ChatGenerationControl>>,
    request_id: RequestId,
) -> Result<bool, IpcError> {
    if request_id.0.trim().is_empty() {
        return Err(IpcError::invalid_request("requestId cannot be empty"));
    }
    if control.cancel(&request_id)?.is_none() {
        return Ok(false);
    }
    core.stop_generation(&request_id.0)
        .map(|_| true)
        .map_err(app_core_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ipc_types::RequestId;
    use std::sync::atomic::Ordering;

    #[test]
    fn chat_generation_control_serializes_and_cancels_requests() -> Result<(), IpcError> {
        let root = tempfile::TempDir::new().map_err(|_| IpcError::internal())?;
        let core = AppCore::initialize(root.path()).map_err(app_core_error)?;
        let control = ChatGenerationControl::new();
        let first = RequestId("first".to_owned());
        let cancellation = control.begin(
            first.clone(),
            core.begin_chat_activity().map_err(app_core_error)?,
        )?;
        assert!(core.begin_chat_activity().is_err());
        assert!(!cancellation.load(Ordering::Acquire));
        assert!(control.cancel(&first)?.is_some());
        assert!(cancellation.load(Ordering::Acquire));
        control.finish(&first);
        assert!(
            control
                .begin(
                    RequestId("second".to_owned()),
                    core.begin_chat_activity().map_err(app_core_error)?,
                )
                .is_ok()
        );
        Ok(())
    }
}
