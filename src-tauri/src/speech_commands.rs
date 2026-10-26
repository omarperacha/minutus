//! Thin speech installation adapters; no capture or inference starts here.

use std::sync::Arc;

use app_core::AppCore;
use ipc_types::{
    DictationAccepted, DictationCommand, DictationEvent, DictationEventsRequest, IpcError,
    SpeechModelRequest, SpeechModelView, StartDictationRequest, ValidateIpc,
};
use tauri::State;

use crate::app_state::SpeechInferenceExecutable;

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn speech_model_selection(
    core: State<'_, Arc<AppCore>>,
) -> Result<SpeechModelRequest, IpcError> {
    core.speech_model_selection().map_err(crate::app_core_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn start_dictation(
    core: State<'_, Arc<AppCore>>,
    executable: State<'_, SpeechInferenceExecutable>,
    request: StartDictationRequest,
) -> Result<DictationAccepted, IpcError> {
    request.validate()?;
    core.inner()
        .start_dictation(request, executable.0.clone())
        .map_err(crate::app_core_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn finish_dictation(
    core: State<'_, Arc<AppCore>>,
    command: DictationCommand,
) -> Result<bool, IpcError> {
    command.validate()?;
    core.finish_dictation(&command)
        .map_err(crate::app_core_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn cancel_dictation(
    core: State<'_, Arc<AppCore>>,
    command: DictationCommand,
) -> Result<bool, IpcError> {
    command.validate()?;
    core.cancel_dictation(&command)
        .map_err(crate::app_core_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn dictation_events(
    core: State<'_, Arc<AppCore>>,
    request: DictationEventsRequest,
) -> Result<Vec<DictationEvent>, IpcError> {
    request.validate()?;
    core.dictation_events(&request)
        .map_err(crate::app_core_error)
}

#[tauri::command]
pub(crate) async fn speech_models(
    core: State<'_, Arc<AppCore>>,
) -> Result<Vec<SpeechModelView>, IpcError> {
    let core = Arc::clone(core.inner());
    tauri::async_runtime::spawn_blocking(move || {
        core.speech_models().map_err(crate::app_core_error)
    })
    .await
    .map_err(|_| IpcError::internal())?
}

#[tauri::command]
pub(crate) async fn install_speech_model(
    core: State<'_, Arc<AppCore>>,
    request: SpeechModelRequest,
) -> Result<(), IpcError> {
    request.validate()?;
    let core = Arc::clone(core.inner());
    tauri::async_runtime::spawn_blocking(move || {
        core.install_speech_model(&request)
            .map_err(crate::app_core_error)
    })
    .await
    .map_err(|_| IpcError::internal())?
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn cancel_speech_download(core: State<'_, Arc<AppCore>>) -> Result<bool, IpcError> {
    core.cancel_speech_download().map_err(crate::app_core_error)
}

#[tauri::command]
pub(crate) async fn remove_speech_model(
    core: State<'_, Arc<AppCore>>,
    request: SpeechModelRequest,
) -> Result<(), IpcError> {
    request.validate()?;
    let core = Arc::clone(core.inner());
    tauri::async_runtime::spawn_blocking(move || {
        core.remove_speech_model(&request)
            .map_err(crate::app_core_error)
    })
    .await
    .map_err(|_| IpcError::internal())?
}
