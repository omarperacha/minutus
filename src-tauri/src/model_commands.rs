//! Local model catalogue, download, and import commands.

use super::{
    AppCore, Arc, CatalogueCompanion, CatalogueInstallProgress, CatalogueModel, CompanionKind,
    DownloadPhase, Emitter, InferenceBackend, IpcError, ModelCompanionKind, ModelDownloadControl,
    ModelDownloadEvent, ModelDownloadPhase, ModelDownloadRequest, ModelInstallReceipt,
    ModelPreferences, ModelSelection, OffsetDateTime, ProjectId, Rfc3339,
    SetConversationModelRequest, SetDefaultModelRequest, State, ValidateIpc, app_core_error,
    current_timestamp,
};

use crate::app_state::ChatGenerationControl;
use crate::app_state::MODEL_DOWNLOAD_PROGRESS_EVENT;

#[tauri::command]
pub(crate) async fn model_catalogue(
    core: State<'_, Arc<AppCore>>,
) -> Result<Vec<CatalogueModel>, IpcError> {
    let core = Arc::clone(core.inner());
    // Legacy provider-backed releases can perform network and lifecycle work
    // here. Never let a catalogue refresh occupy the native event loop.
    tauri::async_runtime::spawn_blocking(move || catalogue_snapshot(&core))
        .await
        .map_err(|_| IpcError::internal())?
}

fn catalogue_snapshot(core: &AppCore) -> Result<Vec<CatalogueModel>, IpcError> {
    core.model_catalogue()
        .models()
        .iter()
        .filter(|model| model.is_manageable())
        .map(|model| {
            let installed = core
                .catalogue_model_installed(model.key())
                .map_err(app_core_error)?;
            let bundle_download_bytes = model
                .ollama_distribution()
                .and_then(model_runtime::OllamaDistribution::tensor_bundle_bytes)
                .or_else(|| {
                    std::iter::once(model.artifact())
                        .chain(
                            model
                                .companions()
                                .iter()
                                .map(model_runtime::CompanionArtifact::artifact),
                        )
                        .try_fold(0_u64, |total, artifact| {
                            total.checked_add(artifact.download_bytes())
                        })
                })
                .ok_or_else(IpcError::internal)?;
            Ok(CatalogueModel {
                id: model.key().id().to_owned(),
                version: model.key().version().to_owned(),
                display_name: model.display_name().to_owned(),
                recommended: model.is_recommended(model_runtime::ModelPlatform::current()),
                provider: match model.backend() {
                    InferenceBackend::LlamaCpp => "llama.cpp",
                    InferenceBackend::Ollama => "Ollama",
                }
                .to_owned(),
                provider_available: core
                    .catalogue_provider_available(model.key())
                    .map_err(app_core_error)?,
                capabilities: capability_labels(model.capabilities()),
                context_limit_tokens: model.capabilities().context_limit_tokens(),
                download_bytes: if model.is_mlx() {
                    bundle_download_bytes
                } else {
                    model.artifact().download_bytes()
                },
                bundle_download_bytes,
                recommended_memory_bytes: model.recommended_memory_bytes(),
                license_url: model.license_url().to_string(),
                usage_policy_url: model.usage_policy_url().to_string(),
                installed,
                registration_state: core
                    .catalogue_registration_state(model.key())
                    .map_err(app_core_error)?,
                external_files: core
                    .catalogue_uses_external_files(model.key())
                    .map_err(app_core_error)?,
                local_import_supported: core
                    .external_import_supported(model.key())
                    .map_err(app_core_error)?,
                chat_selectable: model.is_integrated(),
                modules: core
                    .catalogue_modules(model.key())
                    .map_err(app_core_error)?,
                companions: model
                    .companions()
                    .iter()
                    .map(|companion| {
                        Ok(CatalogueCompanion {
                            artifact_id: companion.artifact().key().id().to_owned(),
                            kind: companion_kind(companion.kind()),
                            required: companion.required_for_setup(),
                            download_bytes: companion.artifact().download_bytes(),
                            memory_requirement_bytes: companion.memory_requirement_bytes(),
                            installed: core
                                .catalogue_companion_installed_by_id(
                                    model.key(),
                                    companion.artifact().key().id(),
                                )
                                .map_err(app_core_error)?,
                        })
                    })
                    .collect::<Result<Vec<_>, IpcError>>()?,
            })
        })
        .collect()
}

#[tauri::command]
pub(crate) async fn model_preferences(
    core: State<'_, Arc<AppCore>>,
) -> Result<ModelPreferences, IpcError> {
    let core = Arc::clone(core.inner());
    tauri::async_runtime::spawn_blocking(move || {
        Ok(ModelPreferences {
            default_model: core
                .ensure_installed_default(&current_timestamp()?)
                .map_err(app_core_error)?,
        })
    })
    .await
    .map_err(|_| IpcError::internal())?
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn set_default_model(
    core: State<'_, Arc<AppCore>>,
    request: SetDefaultModelRequest,
) -> Result<ModelPreferences, IpcError> {
    validate_selection(&request.selection)?;
    core.set_default_model(&request.selection, &current_timestamp()?)
        .map_err(app_core_error)?;
    Ok(ModelPreferences {
        default_model: Some(request.selection),
    })
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn project_model(
    core: State<'_, Arc<AppCore>>,
    project_id: ProjectId,
) -> Result<Option<ModelSelection>, IpcError> {
    project_id.validate()?;
    core.project_model(&project_id).map_err(app_core_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn set_project_model(
    core: State<'_, Arc<AppCore>>,
    project_id: ProjectId,
    selection: Option<ModelSelection>,
) -> Result<Option<ModelSelection>, IpcError> {
    project_id.validate()?;
    if let Some(selection) = selection.as_ref() {
        validate_selection(selection)?;
    }
    core.set_project_model(&project_id, selection.as_ref(), &current_timestamp()?)
        .map_err(app_core_error)?;
    Ok(selection)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn conversation_model(
    core: State<'_, Arc<AppCore>>,
    conversation_id: ipc_types::ConversationId,
) -> Result<Option<ModelSelection>, IpcError> {
    conversation_id.validate()?;
    core.conversation_model(&conversation_id)
        .map_err(app_core_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn set_conversation_model(
    core: State<'_, Arc<AppCore>>,
    generation: State<'_, Arc<ChatGenerationControl>>,
    request: SetConversationModelRequest,
) -> Result<ModelSelection, IpcError> {
    request.conversation_id.validate()?;
    validate_selection(&request.selection)?;
    if generation.is_active()? {
        return Err(IpcError::invalid_request(
            "Wait for the active response to finish before switching models",
        ));
    }
    if !core
        .set_conversation_model(&request.conversation_id, &request.selection)
        .map_err(app_core_error)?
        && core
            .conversation(&request.conversation_id)
            .map_err(app_core_error)?
            .is_none()
    {
        return Err(IpcError::invalid_request("The conversation is unavailable"));
    }
    Ok(request.selection)
}

fn validate_selection(selection: &ModelSelection) -> Result<(), IpcError> {
    if selection.model_id.trim() != selection.model_id
        || selection.model_id.is_empty()
        || selection.model_version.trim() != selection.model_version
        || selection.model_version.is_empty()
        || selection.model_id.len() > 256
        || selection.model_version.len() > 256
    {
        return Err(IpcError::invalid_request("The model selection is invalid"));
    }
    Ok(())
}

fn capability_labels(capabilities: &model_runtime::ModelCapabilities) -> Vec<String> {
    let mut labels = vec!["Text chat".to_owned()];
    if capabilities.native_image_input() {
        labels.push("Image understanding".to_owned());
    }
    if capabilities.structured_tool_calls() {
        labels.push("Structured tools".to_owned());
    }
    if capabilities.separate_reasoning_output() {
        labels.push("Separate reasoning".to_owned());
    }
    labels.push(format!(
        "{}K context",
        capabilities.context_limit_tokens() / 1024
    ));
    labels
}

#[tauri::command]
pub(crate) async fn download_catalogue_model(
    app: tauri::AppHandle,
    core: State<'_, Arc<AppCore>>,
    control: State<'_, Arc<ModelDownloadControl>>,
    request: ModelDownloadRequest,
) -> Result<ModelInstallReceipt, IpcError> {
    request.validate()?;
    let key = core
        .model_catalogue()
        .models()
        .iter()
        .find(|model| {
            model.key().id() == request.model_id && model.key().version() == request.model_version
        })
        .map(|model| model.key().clone())
        .ok_or_else(|| IpcError::invalid_request("The requested model release is unavailable"))?;
    let installed_at = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|_| IpcError::internal())?;
    let cancellation = control.begin()?;
    let core = Arc::clone(core.inner());
    let control = Arc::clone(control.inner());
    let model_id = request.model_id;
    let model_version = request.model_version;
    let companion_artifact_ids = request.companion_artifact_ids;
    let event_app = app.clone();
    let event_model_id = model_id.clone();
    let event_model_version = model_version.clone();

    tauri::async_runtime::spawn_blocking(move || {
        let result = core.install_catalogue_model(
            &key,
            &companion_artifact_ids,
            &cancellation,
            |progress| {
                let _ = event_app.emit(
                    MODEL_DOWNLOAD_PROGRESS_EVENT,
                    progress_event(&event_model_id, &event_model_version, &progress),
                );
            },
            &installed_at,
        );
        control.finish();
        result
            .and_then(|installed| {
                core.ensure_installed_default(&installed_at)?;
                Ok(ModelInstallReceipt {
                    model_id,
                    model_version,
                    installed_bytes: installed.installed_bytes(),
                    artifact_ids: installed.artifact_ids().to_vec(),
                })
            })
            .map_err(app_core_error)
    })
    .await
    .map_err(|_| IpcError::internal())?
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn cancel_model_download(
    control: State<'_, Arc<ModelDownloadControl>>,
) -> Result<bool, IpcError> {
    control.cancel()
}

pub(crate) fn progress_event(
    model_id: &str,
    model_version: &str,
    progress: &CatalogueInstallProgress,
) -> ModelDownloadEvent {
    let artifact = progress.artifact();
    ModelDownloadEvent {
        model_id: model_id.to_owned(),
        model_version: model_version.to_owned(),
        artifact_id: progress.artifact_id().to_owned(),
        companion_kind: progress.companion_kind().map(companion_kind),
        downloaded_bytes: artifact.downloaded_bytes(),
        total_bytes: artifact.total_bytes(),
        remaining_bytes: artifact.remaining_bytes(),
        resumed: artifact.resumed(),
        phase: match artifact.phase() {
            DownloadPhase::Downloading => ModelDownloadPhase::Downloading,
            DownloadPhase::Verifying => ModelDownloadPhase::Verifying,
            DownloadPhase::Installing => ModelDownloadPhase::Installing,
        },
        aggregate_downloaded_bytes: progress.aggregate_downloaded_bytes(),
        aggregate_total_bytes: progress.aggregate_total_bytes(),
        aggregate_remaining_bytes: progress.aggregate_remaining_bytes(),
    }
}

pub(crate) const fn companion_kind(kind: CompanionKind) -> ModelCompanionKind {
    match kind {
        CompanionKind::VisionProjector => ModelCompanionKind::VisionProjector,
        CompanionKind::DraftModel => ModelCompanionKind::DraftModel,
    }
}
