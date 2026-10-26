//! Project current availability without erasing prior successful registration.
use crate::{AppCore, AppCoreError};
use app_storage::ExternalModelRegistration;
use ipc_types::ModelRegistrationState;
use model_runtime::{ExternalModelError, ModelKey, ModelManifest, external_file_stamp};
use std::path::Path;

use super::runtime::bundle_artifacts;
impl AppCore {
    /// Durable registration plus current availability; missing files never erase history.
    ///
    /// # Errors
    /// Returns catalogue or persistence failures.
    pub fn catalogue_registration_state(
        &self,
        key: &ModelKey,
    ) -> Result<ModelRegistrationState, AppCoreError> {
        let model = self
            .model_catalogue
            .model(key)
            .ok_or(AppCoreError::CatalogueModelNotFound)?;
        if let Some(registration) = self.external_registration(model)? {
            return Ok(external_state(
                model,
                &registration,
                self.external_model_access.as_ref(),
            ));
        }
        if self.catalogue_model_installed(key)? {
            return Ok(ModelRegistrationState::Available);
        }
        let recorded = self
            .with_storage(|storage| storage.model(&model.artifact().key().to_string()))?
            .is_some()
            || self.with_storage(|storage| {
                storage.provider_model_installed(&ipc_types::ModelSelection {
                    model_id: key.id().to_owned(),
                    model_version: key.version().to_owned(),
                })
            })?;
        Ok(if recorded {
            ModelRegistrationState::Unavailable
        } else {
            ModelRegistrationState::NotInstalled
        })
    }
}
pub(super) fn require_available(state: ModelRegistrationState) -> Result<(), ExternalModelError> {
    match state {
        ModelRegistrationState::Available => Ok(()),
        ModelRegistrationState::VerificationFailed => Err(ExternalModelError::Changed),
        _ => Err(ExternalModelError::Unavailable),
    }
}

pub(super) fn external_state(
    model: &ModelManifest,
    registration: &ExternalModelRegistration,
    access: &dyn super::ExternalModelAccess,
) -> ModelRegistrationState {
    let artifacts = bundle_artifacts(model);
    if registration.selection.model_id != model.key().id()
        || registration.selection.model_version != model.key().version()
        || registration.verification_failed
    {
        return ModelRegistrationState::VerificationFailed;
    }
    let has_primary = registration
        .files
        .iter()
        .any(|file| file.artifact_id == model.artifact().key().id());
    let missing_required = !has_primary
        || model
            .companions()
            .iter()
            .filter(|companion| companion.required_for_setup())
            .any(|companion| {
                !registration
                    .files
                    .iter()
                    .any(|file| file.artifact_id == companion.artifact().key().id())
            });
    let mut state = ModelRegistrationState::Available;
    for file in &registration.files {
        let Some(artifact) = artifacts
            .iter()
            .find(|artifact| artifact.key().id() == file.artifact_id)
        else {
            return ModelRegistrationState::VerificationFailed;
        };
        if artifact.sha256() != file.sha256 || artifact.installed_bytes() != file.bytes {
            return ModelRegistrationState::VerificationFailed;
        }
        if super::access::restore_file(access, file).is_err() {
            state = ModelRegistrationState::Unavailable;
            continue;
        }
        match external_file_stamp(Path::new(&file.path)) {
            Err(_) => state = ModelRegistrationState::Unavailable,
            Ok((bytes, modified)) if bytes != file.bytes || modified != file.modified_nanos => {
                if state != ModelRegistrationState::Unavailable {
                    state = ModelRegistrationState::VerificationFailed;
                }
            }
            Ok(_) => {}
        }
    }
    if state == ModelRegistrationState::Available && missing_required {
        ModelRegistrationState::Incomplete
    } else {
        state
    }
}
