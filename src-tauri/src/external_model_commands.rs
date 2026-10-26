//! Native-selected external model references. No frontend-supplied filesystem paths.
use crate::{app_core_error, app_state::ModelDownloadControl, current_timestamp};
use app_core::AppCore;
use ipc_types::{ExternalModelProgress, IpcError, ModelSelection};
use std::sync::Arc;
use tauri::{Emitter, State};
use tauri_plugin_dialog::DialogExt;

struct ModelFileOperation(Arc<ModelDownloadControl>);
impl Drop for ModelFileOperation {
    fn drop(&mut self) {
        self.0.finish();
    }
}

fn model_key(
    core: &AppCore,
    selection: &ModelSelection,
) -> Result<model_runtime::ModelKey, IpcError> {
    core.model_catalogue()
        .models()
        .iter()
        .find(|model| {
            model.is_manageable()
                && model.key().id() == selection.model_id
                && model.key().version() == selection.model_version
        })
        .map(|model| model.key().clone())
        .ok_or_else(|| IpcError::invalid_request("The requested model release is unavailable"))
}

#[tauri::command]
pub(crate) async fn register_external_model(
    app: tauri::AppHandle,
    core: State<'_, Arc<AppCore>>,
    control: State<'_, Arc<ModelDownloadControl>>,
    selection: ModelSelection,
    artifact_id: String,
) -> Result<bool, IpcError> {
    let key = model_key(&core, &selection)?;
    if !core
        .external_import_supported(&key)
        .map_err(app_core_error)?
    {
        return Err(IpcError::invalid_request(
            "Local file import is unavailable for this profile",
        ));
    }
    control.begin()?;
    let core = Arc::clone(core.inner());
    let control = Arc::clone(control.inner());
    tauri::async_runtime::spawn_blocking(move || {
        let _operation = ModelFileOperation(control);
        let module = core
            .catalogue_modules(&key)
            .map_err(app_core_error)?
            .into_iter()
            .find(|module| module.artifact_id == artifact_id)
            .ok_or_else(|| IpcError::invalid_request("The requested module is unavailable"))?;
        let requirement = if module.required {
            "Required"
        } else {
            "Optional"
        };
        // Digest-only Ollama blobs are valid selections: do not filter extensions.
        let title = format!(
            "Import {} ({requirement}) — {}",
            module.display_name, module.file_name
        );
        let Some(selected) = app.dialog().file().set_title(title).blocking_pick_file() else {
            return Ok(false);
        };
        let path = selected
            .into_path()
            .map_err(|_| IpcError::invalid_request("Choose a local model file"))?;
        let bookmark = crate::model_file_access::bookmark_for_import(&path)?;
        let mut last_reported = 0;
        core.register_catalogue_module_with_access(
            &key,
            &artifact_id,
            &path,
            bookmark,
            |verified_bytes, total_bytes| {
                if verified_bytes != 0
                    && verified_bytes != total_bytes
                    && verified_bytes.saturating_sub(last_reported) < 16 * 1024 * 1024
                {
                    return;
                }
                last_reported = verified_bytes;
                let _ = app.emit(
                    "external-model-progress",
                    ExternalModelProgress {
                        model_id: selection.model_id.clone(),
                        artifact_id: artifact_id.clone(),
                        phase: ipc_types::ModelDownloadPhase::Verifying,
                        verified_bytes,
                        total_bytes,
                    },
                );
            },
        )
        .map_err(app_core_error)?;
        core.ensure_installed_default(&current_timestamp()?)
            .map_err(app_core_error)?;
        Ok(true)
    })
    .await
    .map_err(|_| IpcError::internal())?
}

#[tauri::command]
pub(crate) async fn download_model_module(
    app: tauri::AppHandle,
    core: State<'_, Arc<AppCore>>,
    control: State<'_, Arc<ModelDownloadControl>>,
    selection: ModelSelection,
    artifact_id: String,
) -> Result<(), IpcError> {
    let key = model_key(&core, &selection)?;
    let cancellation = control.begin()?;
    let core = Arc::clone(core.inner());
    let control = Arc::clone(control.inner());
    tauri::async_runtime::spawn_blocking(move || {
        let _operation = ModelFileOperation(control);
        core.download_catalogue_module(&key, &artifact_id, &cancellation, |progress| {
            let _ = app.emit(
                "external-model-progress",
                ExternalModelProgress {
                    model_id: selection.model_id.clone(),
                    artifact_id: artifact_id.clone(),
                    phase: match progress.phase() {
                        model_runtime::DownloadPhase::Downloading => {
                            ipc_types::ModelDownloadPhase::Downloading
                        }
                        model_runtime::DownloadPhase::Verifying => {
                            ipc_types::ModelDownloadPhase::Verifying
                        }
                        model_runtime::DownloadPhase::Installing => {
                            ipc_types::ModelDownloadPhase::Installing
                        }
                    },
                    verified_bytes: progress.downloaded_bytes(),
                    total_bytes: progress.total_bytes(),
                },
            );
        })
        .map_err(app_core_error)?;
        core.ensure_installed_default(&current_timestamp()?)
            .map_err(app_core_error)?;
        Ok(())
    })
    .await
    .map_err(|_| IpcError::internal())?
}

#[tauri::command]
pub(crate) async fn forget_external_model(
    core: State<'_, Arc<AppCore>>,
    control: State<'_, Arc<ModelDownloadControl>>,
    selection: ModelSelection,
) -> Result<(), IpcError> {
    let key = model_key(&core, &selection)?;
    control.begin()?;
    let core = Arc::clone(core.inner());
    let control = Arc::clone(control.inner());
    tauri::async_runtime::spawn_blocking(move || {
        let _operation = ModelFileOperation(control);
        core.forget_external_model(&key).map_err(app_core_error)
    })
    .await
    .map_err(|_| IpcError::internal())?
}
