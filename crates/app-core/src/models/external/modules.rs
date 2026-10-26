//! Independent module verification, mixed-source setup and catalogue projection.
use crate::{AppCore, AppCoreError};
use app_storage::{ExternalModelFile, ExternalModelRegistration};
use ipc_types::{CatalogueModule, ModelModuleSource, ModelRegistrationState, ModelSelection};
use model_runtime::{
    CompanionKind, DownloadCancellation, DownloadProgress, ExternalModelError, InferenceBackend,
    ModelArtifact, ModelKey, ModelManifest, external_file_stamp, verify_external_file,
};
use std::path::{Path, PathBuf};

use super::runtime::bundle_artifacts;

impl AppCore {
    /// Projects module requirements and individual availability without exposing paths.
    ///
    /// # Errors
    /// Returns catalogue or storage failures.
    pub fn catalogue_modules(&self, key: &ModelKey) -> Result<Vec<CatalogueModule>, AppCoreError> {
        let model = self
            .model_catalogue
            .model(key)
            .ok_or(AppCoreError::CatalogueModelNotFound)?;
        let files = self.module_files(model)?;
        let verification_failed = self
            .external_registration(model)?
            .is_some_and(|record| record.verification_failed);
        Ok(bundle_artifacts(model)
            .into_iter()
            .map(|artifact| {
                let companion = model.companion(artifact.key().id());
                let file = files
                    .iter()
                    .find(|file| file.artifact_id == artifact.key().id());
                CatalogueModule {
                    artifact_id: artifact.key().id().to_owned(),
                    display_name: companion.map_or_else(
                        || {
                            model
                                .display_name()
                                .split(" · ")
                                .next()
                                .unwrap_or(model.display_name())
                                .to_owned()
                        },
                        |companion| match companion.kind() {
                            CompanionKind::VisionProjector => "Image projector".to_owned(),
                            CompanionKind::DraftModel => "Faster generation draft".to_owned(),
                        },
                    ),
                    file_name: artifact.file_name().to_owned(),
                    required: companion
                        .is_none_or(model_runtime::CompanionArtifact::required_for_setup),
                    download_bytes: artifact.download_bytes(),
                    registration_state: file.map_or(ModelRegistrationState::NotInstalled, |file| {
                        if verification_failed {
                            ModelRegistrationState::VerificationFailed
                        } else if super::access::restore_file(
                            self.external_model_access.as_ref(),
                            file,
                        )
                        .is_err()
                        {
                            ModelRegistrationState::Unavailable
                        } else {
                            module_state(artifact, file)
                        }
                    }),
                    source: file.map(|file| {
                        if self.is_managed_module(artifact, Path::new(&file.path)) {
                            ModelModuleSource::Downloaded
                        } else {
                            ModelModuleSource::Imported
                        }
                    }),
                }
            })
            .collect())
    }

    /// Verifies and persists one selected module immediately. Siblings are retained.
    ///
    /// # Errors
    /// Rejects unknown artifacts, invalid content and unsupported profiles. A failure
    /// never replaces an existing registration; external bytes are never modified.
    pub fn register_catalogue_module(
        &self,
        key: &ModelKey,
        artifact_id: &str,
        path: &Path,
        progress: impl FnMut(u64, u64),
    ) -> Result<(), AppCoreError> {
        self.register_catalogue_module_with_access(key, artifact_id, path, None, progress)
    }

    /// Registers one module with opaque native read authority after content verification.
    ///
    /// # Errors
    /// Returns verification, access restoration or persistence failures. Existing
    /// references and their authority remain intact when registration fails.
    pub fn register_catalogue_module_with_access(
        &self,
        key: &ModelKey,
        artifact_id: &str,
        path: &Path,
        access_bookmark: Option<Vec<u8>>,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<(), AppCoreError> {
        let _activity = self.foreground_activity.begin_chat()?;
        let model = self.module_model(key)?;
        let artifact = module_artifact(model, artifact_id)?;
        let mut file = verified_file(artifact, path, |bytes| {
            progress(bytes, artifact.installed_bytes());
        })?;
        file.access_bookmark = access_bookmark;
        let already_registered = self
            .external_registration(model)?
            .is_some_and(|registration| {
                registration.files.iter().any(|retained| {
                    retained.path == file.path && retained.access_bookmark == file.access_bookmark
                })
            });
        super::access::restore_file(self.external_model_access.as_ref(), &file)?;
        if let Err(error) = self.publish_module(model, file.clone()) {
            if !already_registered {
                self.external_model_access
                    .release(Path::new(&file.path), file.access_bookmark.as_deref());
            }
            return Err(error);
        }
        Ok(())
    }

    /// Downloads only the selected module into private managed storage.
    ///
    /// # Errors
    /// Returns transfer, cancellation, verification or persistence failures. Existing
    /// imported siblings remain in place and are never downloaded or copied.
    pub fn download_catalogue_module(
        &self,
        key: &ModelKey,
        artifact_id: &str,
        cancellation: &DownloadCancellation,
        mut progress: impl FnMut(DownloadProgress),
    ) -> Result<(), AppCoreError> {
        let _activity = self.foreground_activity.begin_chat()?;
        let model = self.module_model(key)?;
        let artifact = module_artifact(model, artifact_id)?;
        let downloaded = self
            .model_downloads
            .download(artifact, cancellation, &mut progress)?;
        // Capture a stable verified-file stamp for the same durable registration
        // format as local imports. Runtime use checks access and file attributes.
        let file = verified_file(artifact, downloaded.path(), |_| {})?;
        if cancellation.is_cancelled() {
            return Err(model_runtime::DownloadError::Cancelled.into());
        }
        self.publish_module(model, file)
    }

    fn module_model(&self, key: &ModelKey) -> Result<&ModelManifest, AppCoreError> {
        let model = self
            .model_catalogue
            .model(key)
            .ok_or(AppCoreError::CatalogueModelNotFound)?;
        if !self.external_import_supported(key)? {
            return Err(ExternalModelError::Unsupported.into());
        }
        Ok(model)
    }

    fn publish_module(
        &self,
        model: &ModelManifest,
        file: ExternalModelFile,
    ) -> Result<(), AppCoreError> {
        let previous = self.external_registration(model)?;
        let mut files = self.module_files(model)?;
        if previous
            .as_ref()
            .is_none_or(|record| record.verification_failed)
        {
            // Legacy managed receipts do not contain stable file stamps. Authenticate
            // their bytes before adopting them into the incremental format.
            for retained in &mut files {
                if retained.artifact_id != file.artifact_id {
                    super::access::restore_file(self.external_model_access.as_ref(), retained)?;
                    let bookmark = retained.access_bookmark.clone();
                    *retained = verified_file(
                        module_artifact(model, &retained.artifact_id)?,
                        Path::new(&retained.path),
                        |_| {},
                    )?;
                    retained.access_bookmark = bookmark;
                }
            }
        }
        files.retain(|retained| retained.artifact_id != file.artifact_id);
        files.push(file);
        let artifacts = bundle_artifacts(model);
        files.sort_by_key(|file| {
            artifacts
                .iter()
                .position(|artifact| artifact.key().id() == file.artifact_id)
        });
        // A sibling can disappear while the newly chosen large file is being hashed.
        // Preserve its durable identity for Locate files, but never declare it ready.
        let registration = ExternalModelRegistration {
            model_id: model.artifact().key().to_string(),
            selection: ModelSelection {
                model_id: model.key().id().to_owned(),
                model_version: model.key().version().to_owned(),
            },
            verification_failed: false,
            files,
        };
        self.model_runtime.unload_model()?;
        self.ollama_shutdown.stop_for_model_change()?;
        if let Some(store) = &self.ollama_models_directory {
            if let Some(previous) = &previous {
                for file in &previous.files {
                    model_runtime::unlink_external_blob(
                        store,
                        Path::new(&file.path),
                        &file.sha256,
                    )?;
                }
            }
            model_runtime::unregister_external_model_metadata(store, model)?;
        }
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

    fn module_files(&self, model: &ModelManifest) -> Result<Vec<ExternalModelFile>, AppCoreError> {
        if let Some(registration) = self.external_registration(model)? {
            return Ok(registration.files);
        }
        let mut files = Vec::new();
        let provider_installed = model.backend() == InferenceBackend::Ollama
            && self.catalogue_model_installed(model.key())?;
        for artifact in bundle_artifacts(model) {
            let recorded = if provider_installed {
                self.ollama_models_directory.as_ref().map(|root| {
                    (
                        root.join("blobs")
                            .join(format!("sha256-{}", artifact.sha256())),
                        artifact.sha256().to_owned(),
                        artifact.installed_bytes(),
                    )
                })
            } else if artifact.key() == model.artifact().key() {
                self.with_storage(|storage| storage.model(&model.artifact().key().to_string()))?
                    .map(|file| {
                        (
                            PathBuf::from(file.local_path),
                            file.content_digest,
                            file.byte_size,
                        )
                    })
            } else {
                self.with_storage(|storage| {
                    storage
                        .model_companion(&model.artifact().key().to_string(), artifact.key().id())
                })?
                .map(|file| {
                    (
                        PathBuf::from(file.local_path),
                        file.content_digest,
                        file.byte_size,
                    )
                })
            };
            if let Some((path, sha256, bytes)) = recorded
                && let Ok((_, modified_nanos)) = external_file_stamp(&path)
            {
                files.push(ExternalModelFile {
                    artifact_id: artifact.key().id().to_owned(),
                    path: path.to_string_lossy().into_owned(),
                    sha256,
                    bytes,
                    modified_nanos,
                    access_bookmark: None,
                });
            }
        }
        Ok(files)
    }

    fn is_managed_module(&self, artifact: &ModelArtifact, path: &Path) -> bool {
        self.model_downloads.is_published(artifact, path)
            || self.ollama_models_directory.as_ref().is_some_and(|root| {
                path == root
                    .join("blobs")
                    .join(format!("sha256-{}", artifact.sha256()))
            })
    }
}

fn module_artifact<'a>(
    model: &'a ModelManifest,
    artifact_id: &str,
) -> Result<&'a ModelArtifact, AppCoreError> {
    bundle_artifacts(model)
        .into_iter()
        .find(|artifact| artifact.key().id() == artifact_id)
        .ok_or(AppCoreError::InvalidCompanionSelection)
}

fn verified_file(
    artifact: &ModelArtifact,
    path: &Path,
    progress: impl FnMut(u64),
) -> Result<ExternalModelFile, AppCoreError> {
    external_file_stamp(path)?;
    let path = path
        .canonicalize()
        .map_err(|_| ExternalModelError::Unavailable)?;
    let verified = verify_external_file(artifact, &path, progress)?;
    Ok(ExternalModelFile {
        artifact_id: artifact.key().id().to_owned(),
        path: verified.path.to_string_lossy().into_owned(),
        sha256: artifact.sha256().to_owned(),
        bytes: artifact.installed_bytes(),
        modified_nanos: verified.modified_nanos,
        access_bookmark: None,
    })
}

fn module_state(artifact: &ModelArtifact, file: &ExternalModelFile) -> ModelRegistrationState {
    if artifact.sha256() != file.sha256 || artifact.installed_bytes() != file.bytes {
        return ModelRegistrationState::VerificationFailed;
    }
    match external_file_stamp(Path::new(&file.path)) {
        Err(_) => ModelRegistrationState::Unavailable,
        Ok((bytes, modified)) if bytes != file.bytes || modified != file.modified_nanos => {
            ModelRegistrationState::VerificationFailed
        }
        Ok(_) => ModelRegistrationState::Available,
    }
}
