//! Resolve import-verified external files using current access and availability checks.
use crate::{AppCore, AppCoreError};
use app_storage::{ModelCompanionMetadata, ModelMetadata};
use model_runtime::{ModelArtifact, ModelManifest};
use std::path::Path;

use super::availability::{external_state, require_available};
impl AppCore {
    pub(crate) fn external_primary(
        &self,
        model: &ModelManifest,
    ) -> Result<Option<ModelMetadata>, AppCoreError> {
        let Some(registration) = self.external_registration(model)? else {
            return Ok(None);
        };
        require_available(external_state(
            model,
            &registration,
            self.external_model_access.as_ref(),
        ))?;
        let file = registration
            .files
            .iter()
            .find(|file| file.artifact_id == model.artifact().key().id())
            .ok_or(AppCoreError::InstalledModelInvalid)?;
        Ok(Some(ModelMetadata {
            id: registration.model_id,
            display_name: model.display_name().to_owned(),
            format: model.artifact().format().as_str().to_owned(),
            local_path: file.path.clone(),
            content_digest: file.sha256.clone(),
            byte_size: file.bytes,
            imported_at: String::new(),
        }))
    }

    pub(in crate::models) fn external_companion(
        &self,
        model: &ModelManifest,
        artifact_id: &str,
    ) -> Result<Option<ModelCompanionMetadata>, AppCoreError> {
        let Some(registration) = self.external_registration(model)? else {
            return Ok(None);
        };
        require_available(external_state(
            model,
            &registration,
            self.external_model_access.as_ref(),
        ))?;
        let companion = model
            .companion(artifact_id)
            .ok_or(AppCoreError::InvalidCompanionSelection)?;
        let Some(file) = registration
            .files
            .iter()
            .find(|file| file.artifact_id == artifact_id)
        else {
            return Ok(None);
        };
        Ok(Some(ModelCompanionMetadata {
            model_id: registration.model_id,
            artifact_id: artifact_id.to_owned(),
            kind: companion.kind().as_str().to_owned(),
            required: companion.required(),
            local_path: file.path.clone(),
            content_digest: file.sha256.clone(),
            byte_size: file.bytes,
            compatibility_version: companion.compatibility_version().to_owned(),
            memory_requirement_bytes: companion.memory_requirement_bytes(),
            installed_at: String::new(),
        }))
    }

    pub(crate) fn prepare_external_runtime_references(
        &self,
        model: &ModelManifest,
    ) -> Result<(), AppCoreError> {
        let Some(registration) = self.external_registration(model)? else {
            return Ok(());
        };
        require_available(external_state(
            model,
            &registration,
            self.external_model_access.as_ref(),
        ))?;
        if let Some(store) = &self.ollama_models_directory {
            for file in &registration.files {
                model_runtime::link_external_blob(store, Path::new(&file.path), &file.sha256)?;
            }
            if model.backend() == model_runtime::InferenceBackend::Ollama {
                model_runtime::register_external_qwen_metadata(store, model)?;
            } else if model.key().id() == "meta-models/muse-glimmer-30b" {
                let with_projector = model
                    .multimodal_profile()
                    .and_then(model_runtime::MultimodalProfile::projector_artifact_id)
                    .is_some_and(|id| registration.files.iter().any(|file| file.artifact_id == id));
                model_runtime::register_external_glimmer_metadata(store, model, with_projector)?;
            }
        }
        Ok(())
    }
}
pub(super) fn bundle_artifacts(model: &ModelManifest) -> Vec<&ModelArtifact> {
    std::iter::once(model.artifact())
        .chain(
            model
                .companions()
                .iter()
                .map(model_runtime::CompanionArtifact::artifact),
        )
        .collect()
}
