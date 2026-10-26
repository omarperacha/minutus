//! Durable registration and explicit removal of external model references.
use crate::{AppCore, AppCoreError};
use app_storage::{ExternalModelFile, ExternalModelRegistration};
use model_runtime::{
    ExternalModelError, ModelKey, ModelManifest, external_file_stamp, verify_external_file,
};
use std::path::{Path, PathBuf};

use super::{
    availability::{external_state, require_available},
    runtime::bundle_artifacts,
};
impl AppCore {
    /// Registers exact catalogue files in place after verifying the complete bundle.
    ///
    /// A failed/cancelled picker never calls this method. Failed verification leaves
    /// any previous registration untouched. No source file is copied or mutated.
    ///
    /// # Errors
    /// Rejects unsupported profiles, mismatched files, inaccessible paths or storage failures.
    pub fn register_external_model(
        &self,
        key: &ModelKey,
        paths: &[Option<PathBuf>],
        mut progress: impl FnMut(u64, u64),
    ) -> Result<(), AppCoreError> {
        let _activity = self.foreground_activity.begin_chat()?;
        let model = self
            .model_catalogue
            .model(key)
            .ok_or(AppCoreError::CatalogueModelNotFound)?;
        if !self.external_import_supported(key)? {
            return Err(ExternalModelError::Unsupported.into());
        }
        let artifacts = bundle_artifacts(model);
        if paths.len() != artifacts.len() {
            return Err(AppCoreError::InvalidCompanionSelection);
        }
        let total = artifacts
            .iter()
            .zip(paths)
            .filter(|(_, path)| path.is_some())
            .map(|(artifact, _)| artifact.installed_bytes())
            .sum();
        let mut completed = 0;
        let mut files = Vec::new();
        for (artifact, path) in artifacts.into_iter().zip(paths) {
            let Some(path) = path else {
                if artifact.key() == model.artifact().key()
                    || model
                        .companion(artifact.key().id())
                        .is_some_and(model_runtime::CompanionArtifact::required_for_setup)
                {
                    return Err(AppCoreError::InvalidCompanionSelection);
                }
                continue;
            };
            // Reject direct symlinks before canonicalizing. Persist the actual chosen file,
            // not a relative path or a mutable alias in the runtime's private store.
            external_file_stamp(path)?;
            let path = path
                .canonicalize()
                .map_err(|_| ExternalModelError::Unavailable)?;
            let verified =
                verify_external_file(artifact, &path, |bytes| progress(completed + bytes, total))?;
            files.push(ExternalModelFile {
                artifact_id: artifact.key().id().to_owned(),
                path: verified.path.to_string_lossy().into_owned(),
                sha256: artifact.sha256().to_owned(),
                bytes: artifact.installed_bytes(),
                modified_nanos: verified.modified_nanos,
                access_bookmark: None,
            });
            completed += artifact.installed_bytes();
        }
        let registration = ExternalModelRegistration {
            model_id: model.artifact().key().to_string(),
            selection: ipc_types::ModelSelection {
                model_id: key.id().to_owned(),
                model_version: key.version().to_owned(),
            },
            verification_failed: false,
            files,
        };
        // Earlier artifacts may move while later companions are being hashed.
        // A first import must not publish a registration that was never available.
        require_available(external_state(
            model,
            &registration,
            self.external_model_access.as_ref(),
        ))?;
        self.model_runtime.unload_model()?;
        self.ollama_shutdown.stop_for_model_change()?;
        let previous = self.external_registration(model)?;
        if let (Some(previous), Some(store)) = (&previous, &self.ollama_models_directory) {
            for file in &previous.files {
                model_runtime::unlink_external_blob(store, Path::new(&file.path), &file.sha256)?;
            }
        }
        if let Some(store) = &self.ollama_models_directory {
            // A replacement may change optional companion membership. Force local
            // tag recreation rather than retaining a previous text-only manifest.
            model_runtime::unregister_external_model_metadata(store, model)?;
        }
        // Provider aliases are prepared lazily before use, using the import-verified registration.
        self.with_storage(|storage| storage.save_external_model(&registration))?;
        if let Some(previous) = previous {
            for file in previous.files {
                if !registration.files.iter().any(|retained| {
                    retained.path == file.path && retained.access_bookmark == file.access_bookmark
                }) {
                    self.external_model_access
                        .release(Path::new(&file.path), file.access_bookmark.as_deref());
                }
            }
        }
        Ok(())
    }

    /// Whether this build can reference this catalogue's local files without copying.
    ///
    /// # Errors
    /// Rejects unknown catalogue keys.
    pub fn external_import_supported(&self, key: &ModelKey) -> Result<bool, AppCoreError> {
        let model = self
            .model_catalogue
            .model(key)
            .ok_or(AppCoreError::CatalogueModelNotFound)?;
        Ok(cfg!(unix)
            && model.is_integrated()
            && !model.is_mlx()
            && self.ollama_models_directory.is_some()
            && self.ollama_shutdown.owns_runtime_store())
    }

    pub(in crate::models) fn external_registration(
        &self,
        model: &ModelManifest,
    ) -> Result<Option<ExternalModelRegistration>, AppCoreError> {
        self.with_storage(|storage| storage.external_model(&model.artifact().key().to_string()))
    }

    /// Whether the current registration references user-owned files.
    ///
    /// # Errors
    /// Returns catalogue or persistence failures.
    pub fn catalogue_uses_external_files(&self, key: &ModelKey) -> Result<bool, AppCoreError> {
        let model = self
            .model_catalogue
            .model(key)
            .ok_or(AppCoreError::CatalogueModelNotFound)?;
        Ok(self.external_registration(model)?.is_some())
    }

    /// Unregisters external files and removes only runtime-owned symlink aliases.
    ///
    /// # Errors
    /// Returns runtime or persistence failures. External targets are never deleted.
    pub fn forget_external_model(&self, key: &ModelKey) -> Result<(), AppCoreError> {
        let _activity = self.foreground_activity.begin_chat()?;
        let model = self
            .model_catalogue
            .model(key)
            .ok_or(AppCoreError::CatalogueModelNotFound)?;
        if let Some(registration) = self.external_registration(model)? {
            self.model_runtime.unload_model()?;
            self.ollama_shutdown.stop_for_model_change()?;
            if let Some(store) = &self.ollama_models_directory {
                for file in &registration.files {
                    model_runtime::unlink_external_blob(
                        store,
                        Path::new(&file.path),
                        &file.sha256,
                    )?;
                }
                model_runtime::unregister_external_model_metadata(store, model)?;
            }
            self.with_storage(|storage| storage.forget_external_model(&registration.model_id))?;
            for file in &registration.files {
                self.external_model_access
                    .release(Path::new(&file.path), file.access_bookmark.as_deref());
            }
        }
        Ok(())
    }
}
