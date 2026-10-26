use crate::{
    AppCore, AppCoreError, ApplicationStore, BTreeMap, CatalogueInstallProgress,
    CatalogueInstallReceipt, CompanionArtifact, CompanionKind, CompanionLoadConfig, ConversationId,
    DownloadCancellation, DownloadError, GLIMMER_IMAGE_TOKENS, GLIMMER_OLLAMA_TAG, ImportProgress,
    ImportedModelArtifact, InferenceBackend, MAX_GENERATION_IMAGES, MAX_PRODUCT_CONTEXT_TOKENS,
    MAX_PRODUCT_OUTPUT_TOKENS, ModelArtifact, ModelCatalogue, ModelCompanionMetadata, ModelKey,
    ModelLoadConfig, ModelMetadata, ModelSelection, OffsetDateTime, OllamaProvider, Path,
    ProjectId, Rfc3339, SettingKey, SettingValue, StorageError, fs, restrict_directory_permissions,
};

impl AppCore {
    /// Fully verifies and loads one installed catalogue model.
    ///
    /// The frontend cannot supply a model path, digest or backend revision;
    /// these are resolved from validated catalogue and Rust-owned metadata.
    ///
    /// # Errors
    ///
    /// Returns an error if the release is absent, unpublished, exceeds product
    /// limits, fails reverification or cannot be loaded safely.
    pub fn load_catalogue_model(
        &self,
        executable: impl AsRef<Path>,
        key: &ModelKey,
        context_tokens: u32,
        output_tokens: u32,
    ) -> Result<(), AppCoreError> {
        self.load_catalogue_model_with_companions(
            executable,
            key,
            context_tokens,
            output_tokens,
            &[],
        )
    }

    /// Fully verifies and loads a catalogue model with selected companion artifacts.
    ///
    /// Required companions are always selected. Optional companions must be named
    /// by their model-local catalogue IDs; paths and integrity values remain Rust-owned.
    ///
    /// # Errors
    ///
    /// Returns an error before sidecar launch if any selected companion is absent,
    /// corrupt, duplicated or outside the model's exact compatibility group.
    pub fn load_catalogue_model_with_companions(
        &self,
        executable: impl AsRef<Path>,
        key: &ModelKey,
        context_tokens: u32,
        output_tokens: u32,
        selected_companion_ids: &[String],
    ) -> Result<(), AppCoreError> {
        validate_inference_limits(context_tokens, output_tokens)?;
        let model = self
            .model_catalogue
            .model(key)
            .ok_or(AppCoreError::CatalogueModelNotFound)?;
        if !model.is_integrated() || model.backend() != model_runtime::InferenceBackend::LlamaCpp {
            return Err(AppCoreError::CatalogueModelNotFound);
        }
        let id = model.artifact().key().to_string();
        let external = self.external_primary(model)?;
        let is_external = external.is_some();
        let metadata = external
            .or(self.with_storage(|storage| storage.model(&id))?)
            .ok_or(AppCoreError::InstalledModelNotFound)?;
        if metadata.content_digest != model.artifact().sha256()
            || metadata.byte_size != model.artifact().installed_bytes()
            || (!is_external
                && !self
                    .model_downloads
                    .is_published(model.artifact(), Path::new(&metadata.local_path)))
        {
            return Err(AppCoreError::InstalledModelInvalid);
        }
        let companions = selected_companions(model, selected_companion_ids)?;
        if companions
            .iter()
            .any(|companion| companion.kind() == CompanionKind::VisionProjector)
        {
            let profile = model
                .multimodal_profile()
                .ok_or(AppCoreError::InvalidCompanionSelection)?;
            if profile.image_tokens() != GLIMMER_IMAGE_TOKENS
                || usize::try_from(profile.max_images()).ok() != Some(MAX_GENERATION_IMAGES)
                || !companions.iter().any(|companion| {
                    companion.kind() == CompanionKind::VisionProjector
                        && Some(companion.artifact().key().id()) == profile.projector_artifact_id()
                })
            {
                return Err(AppCoreError::InvalidCompanionSelection);
            }
        }
        let companion_configs = companions
            .iter()
            .map(|companion| {
                let companion_metadata = self
                    .external_companion(model, companion.artifact().key().id())?
                    .or(self.with_storage(|storage| {
                        storage.model_companion(&id, companion.artifact().key().id())
                    })?)
                    .ok_or(AppCoreError::InstalledModelNotFound)?;
                if companion_metadata.kind != companion.kind().as_str()
                    || companion_metadata.content_digest != companion.artifact().sha256()
                    || companion_metadata.byte_size != companion.artifact().installed_bytes()
                    || companion_metadata.compatibility_version != companion.compatibility_version()
                    || (!is_external
                        && !self.model_downloads.is_published(
                            companion.artifact(),
                            Path::new(&companion_metadata.local_path),
                        ))
                {
                    return Err(AppCoreError::InstalledModelInvalid);
                }
                Ok(CompanionLoadConfig::new(
                    companion.kind(),
                    companion_metadata.local_path,
                    companion_metadata.content_digest,
                    companion_metadata.byte_size,
                    companion_metadata.compatibility_version,
                ))
            })
            .collect::<Result<Vec<_>, AppCoreError>>()?;
        let mut config = ModelLoadConfig::new(
            executable.as_ref(),
            metadata.local_path,
            metadata.content_digest,
            metadata.byte_size,
        )
        .with_limits(context_tokens, output_tokens)
        .with_backend_compatibility_version(model.backend_compatibility_version())
        .with_execution_profile(
            model.key().id(),
            model.key().version(),
            model.capabilities().clone(),
        );
        if !companion_configs.is_empty() {
            config =
                config.with_companions(model.companion_compatibility_version(), companion_configs);
        }
        self.model_runtime.load_model(config).map_err(Into::into)
    }

    /// Fully verifies and loads one imported model from managed storage.
    ///
    /// # Errors
    ///
    /// Returns an error if its opaque record is absent, outside managed model
    /// storage, incompatible, corrupt or exceeds product limits.
    pub fn load_imported_model(
        &self,
        executable: impl AsRef<Path>,
        model_id: &str,
        context_tokens: u32,
        output_tokens: u32,
    ) -> Result<(), AppCoreError> {
        validate_inference_limits(context_tokens, output_tokens)?;
        if !model_id.starts_with("imported:") {
            return Err(AppCoreError::InstalledModelNotFound);
        }
        let metadata = self
            .with_storage(|storage| storage.model(model_id))?
            .ok_or(AppCoreError::InstalledModelNotFound)?;
        if metadata.format != "gguf"
            || !self.model_imports.is_published(
                Path::new(&metadata.local_path),
                &metadata.content_digest,
                metadata.byte_size,
            )
        {
            return Err(AppCoreError::InstalledModelInvalid);
        }
        let config = ModelLoadConfig::new(
            executable.as_ref(),
            metadata.local_path,
            metadata.content_digest,
            metadata.byte_size,
        )
        .with_limits(context_tokens, output_tokens);
        self.model_runtime.load_model(config).map_err(Into::into)
    }

    /// Returns the bundled catalogue after strict Rust-owned validation.
    #[must_use]
    pub const fn model_catalogue(&self) -> &ModelCatalogue {
        &self.model_catalogue
    }

    /// Reports whether one catalogue release has atomically published metadata and bytes.
    ///
    /// # Errors
    ///
    /// Returns an error when the release is unknown or persisted metadata cannot be read.
    pub fn catalogue_model_installed(&self, key: &ModelKey) -> Result<bool, AppCoreError> {
        let model = self
            .model_catalogue
            .model(key)
            .ok_or(AppCoreError::CatalogueModelNotFound)?;
        if self.external_registration(model)?.is_some() {
            return Ok(self.catalogue_registration_state(key)?
                == ipc_types::ModelRegistrationState::Available);
        }
        if model.backend() == InferenceBackend::Ollama {
            let selection = ModelSelection {
                model_id: model.key().id().to_owned(),
                model_version: model.key().version().to_owned(),
            };
            let provider_result = match self.with_ollama_provider(|provider| {
                if provider.check_available().is_err() {
                    return Ok(None);
                }
                Ok(Some(provider.verify_model(model).is_ok()))
            }) {
                Ok(result) => result,
                Err(AppCoreError::Ollama(_) | AppCoreError::OllamaSidecar(_)) => None,
                Err(error) => return Err(error),
            };
            return match provider_result {
                Some(true) => {
                    let installed_at = OffsetDateTime::now_utc()
                        .format(&Rfc3339)
                        .map_err(|_| AppCoreError::InstalledModelInvalid)?;
                    let installed_bytes = complete_model_bytes(model)?;
                    self.with_storage(|storage| {
                        storage.save_provider_model_install(
                            &selection,
                            installed_bytes,
                            &installed_at,
                        )
                    })?;
                    Ok(true)
                }
                Some(false) => Ok(false),
                None => self.with_storage(|storage| storage.provider_model_installed(&selection)),
            };
        }
        let id = model.artifact().key().to_string();
        let Some(metadata) = self.with_storage(|storage| storage.model(&id))? else {
            return Ok(false);
        };
        let primary_installed = metadata.content_digest == model.artifact().sha256()
            && metadata.byte_size == model.artifact().installed_bytes()
            && self
                .model_downloads
                .is_published(model.artifact(), Path::new(&metadata.local_path));
        if !primary_installed {
            return Ok(false);
        }
        for companion in model
            .companions()
            .iter()
            .filter(|companion| companion.required_for_setup())
        {
            if !self.catalogue_companion_installed(model, companion)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Reports whether the provider for one release is currently available.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown release or unavailable internal state.
    pub fn catalogue_provider_available(&self, key: &ModelKey) -> Result<bool, AppCoreError> {
        let model = self
            .model_catalogue
            .model(key)
            .ok_or(AppCoreError::CatalogueModelNotFound)?;
        if model.backend() == InferenceBackend::Ollama {
            return match self
                .with_ollama_provider(|provider| Ok(provider.check_available().is_ok()))
            {
                Ok(available) => Ok(available),
                Err(AppCoreError::Ollama(_) | AppCoreError::OllamaSidecar(_)) => Ok(false),
                Err(error) => Err(error),
            };
        }
        Ok(true)
    }

    pub(crate) fn validated_selection(
        &self,
        selection: &ModelSelection,
    ) -> Result<&model_runtime::ModelManifest, AppCoreError> {
        let model = self
            .model_catalogue
            .models()
            .iter()
            .find(|model| {
                model.key().id() == selection.model_id
                    && model.key().version() == selection.model_version
            })
            .ok_or(AppCoreError::CatalogueModelNotFound)?;
        // Preserve actionable external-file failures if a path disappears between
        // the UI availability snapshot and generation/selection admission.
        self.external_primary(model)?;
        if !model.is_manageable() || !self.catalogue_model_installed(model.key())? {
            return Err(AppCoreError::InstalledModelNotFound);
        }
        Ok(model)
    }

    /// Returns the default model assigned only when a new chat is created.
    ///
    /// # Errors
    ///
    /// Returns an error when persistence is unavailable or malformed.
    pub fn default_model(&self) -> Result<Option<ModelSelection>, AppCoreError> {
        let Some(SettingValue::SelectedModel(value)) =
            self.with_storage(|storage| storage.setting(SettingKey::SelectedModel))?
        else {
            return Ok(None);
        };
        value
            .map(|value| {
                serde_json::from_str(&value).map_err(|_| AppCoreError::InstalledModelInvalid)
            })
            .transpose()
    }

    /// Persists a completely installed catalogue release as the new-chat default.
    ///
    /// # Errors
    ///
    /// Returns an error when the release is absent, unavailable, or cannot be persisted.
    pub fn set_default_model(
        &self,
        selection: &ModelSelection,
        updated_at: &str,
    ) -> Result<(), AppCoreError> {
        self.validated_selection(selection)?;
        let encoded =
            serde_json::to_string(selection).map_err(|_| AppCoreError::InstalledModelInvalid)?;
        self.with_storage(|storage| {
            storage.set_setting(&SettingValue::SelectedModel(Some(encoded)), updated_at)
        })
    }

    /// Returns the optional installed-model preference for one project.
    ///
    /// # Errors
    ///
    /// Returns an error when the project or persisted identity is unavailable.
    pub fn project_model(
        &self,
        project_id: &ProjectId,
    ) -> Result<Option<ModelSelection>, AppCoreError> {
        if self
            .with_storage(|storage| storage.project(project_id))?
            .is_none()
        {
            return Err(AppCoreError::ProjectNotFound);
        }
        self.with_storage(|storage| storage.project_model(project_id))
    }

    /// Replaces or clears the installed-model preference for one project.
    ///
    /// Existing chats retain their exact model binding; newly created chats
    /// inherit this preference or the application default when it is cleared.
    ///
    /// # Errors
    ///
    /// Returns an error when the project or selected release is unavailable.
    pub fn set_project_model(
        &self,
        project_id: &ProjectId,
        selection: Option<&ModelSelection>,
        updated_at: &str,
    ) -> Result<bool, AppCoreError> {
        if let Some(selection) = selection {
            self.validated_selection(selection)?;
        }
        if self
            .with_storage(|storage| storage.project(project_id))?
            .is_none()
        {
            return Err(AppCoreError::ProjectNotFound);
        }
        self.with_storage(|storage| storage.set_project_model(project_id, selection, updated_at))
    }

    /// Persists the platform-preferred installed model, or the sole installed model,
    /// when the user has not selected one yet.
    ///
    /// # Errors
    ///
    /// Returns an error when installation state or settings persistence fails.
    pub fn ensure_installed_default(
        &self,
        updated_at: &str,
    ) -> Result<Option<ModelSelection>, AppCoreError> {
        let configured = self.default_model()?;
        if let Some(selection) = &configured
            && self.model_catalogue.models().iter().any(|model| {
                model.key().id() == selection.model_id
                    && model.key().version() == selection.model_version
                    && model.is_manageable()
            })
        {
            return Ok(configured);
        }
        let mut installed = Vec::new();
        for model in self
            .model_catalogue
            .models()
            .iter()
            .filter(|model| model.is_manageable())
        {
            if self.catalogue_model_installed(model.key())? {
                installed.push((
                    ModelSelection {
                        model_id: model.key().id().to_owned(),
                        model_version: model.key().version().to_owned(),
                    },
                    true,
                ));
            }
        }
        let Some(selection) = resolve_new_conversation_selection(None, &installed) else {
            return Ok(None);
        };
        // An unavailable experimental preference stays saved for experimental
        // mode; normal mode reports the effective installed default instead.
        if configured.is_none() {
            self.set_default_model(&selection, updated_at)?;
        }
        Ok(Some(selection))
    }

    /// Loads the exact release currently bound to a chat.
    ///
    /// # Errors
    ///
    /// Returns an error when persistence is unavailable or malformed.
    pub fn conversation_model(
        &self,
        conversation_id: &ConversationId,
    ) -> Result<Option<ModelSelection>, AppCoreError> {
        self.with_storage(|storage| storage.conversation_model(conversation_id))
    }

    /// Rebinds a chat and atomically drops model-derived semantic checkpoints.
    ///
    /// # Errors
    ///
    /// Returns an error when the release is unavailable or persistence fails.
    pub fn set_conversation_model(
        &self,
        conversation_id: &ConversationId,
        selection: &ModelSelection,
    ) -> Result<bool, AppCoreError> {
        self.validated_selection(selection)?;
        self.with_storage(|storage| storage.set_conversation_model(conversation_id, selection))
    }

    pub(crate) fn new_conversation_model(
        &self,
        project_id: &ProjectId,
    ) -> Result<Option<ModelSelection>, AppCoreError> {
        let preferred = self.project_model(project_id)?.or(self.default_model()?);
        self.resolve_new_conversation_model(preferred.as_ref())
    }

    pub(crate) fn new_default_conversation_model(
        &self,
    ) -> Result<Option<ModelSelection>, AppCoreError> {
        let preferred = self.default_model()?;
        self.resolve_new_conversation_model(preferred.as_ref())
    }

    fn resolve_new_conversation_model(
        &self,
        preferred: Option<&ModelSelection>,
    ) -> Result<Option<ModelSelection>, AppCoreError> {
        let candidates = self
            .model_catalogue
            .models()
            .iter()
            .filter(|model| model.is_manageable())
            .map(|model| {
                Ok((
                    ModelSelection {
                        model_id: model.key().id().to_owned(),
                        model_version: model.key().version().to_owned(),
                    },
                    self.catalogue_model_installed(model.key())?,
                ))
            })
            .collect::<Result<Vec<_>, AppCoreError>>()?;
        Ok(resolve_new_conversation_selection(preferred, &candidates))
    }

    /// Reports whether one companion has matching metadata and published bytes.
    ///
    /// # Errors
    ///
    /// Returns an error when the model or artifact is unknown or storage fails.
    pub fn catalogue_companion_installed_by_id(
        &self,
        key: &ModelKey,
        artifact_id: &str,
    ) -> Result<bool, AppCoreError> {
        let model = self
            .model_catalogue
            .model(key)
            .ok_or(AppCoreError::CatalogueModelNotFound)?;
        let companion = model
            .companion(artifact_id)
            .ok_or(AppCoreError::InvalidCompanionSelection)?;
        if let Some(registration) = self.external_registration(model)? {
            return Ok(self.catalogue_registration_state(key)?
                == ipc_types::ModelRegistrationState::Available
                && registration
                    .files
                    .iter()
                    .any(|file| file.artifact_id == artifact_id));
        }
        if model.backend() == InferenceBackend::Ollama {
            return self.with_ollama_provider(|provider| Ok(provider.verify_model(model).is_ok()));
        }
        self.catalogue_companion_installed(model, companion)
    }

    /// Downloads and registers one exact catalogue release after verification.
    ///
    /// Progress snapshots include downloaded, total and remaining bytes. The
    /// managed artifact becomes visible in installed-model metadata only after
    /// its complete SHA-256 has passed and the file was atomically published.
    ///
    /// # Errors
    ///
    /// Returns an error when the release is not in the validated catalogue, the
    /// transfer cannot complete safely, integrity verification fails or model
    /// metadata cannot be committed.
    pub fn install_catalogue_model(
        &self,
        key: &ModelKey,
        selected_companion_ids: &[String],
        cancellation: &DownloadCancellation,
        mut progress: impl FnMut(CatalogueInstallProgress),
        installed_at: &str,
    ) -> Result<CatalogueInstallReceipt, AppCoreError> {
        let model = self
            .model_catalogue
            .model(key)
            .ok_or(AppCoreError::CatalogueModelNotFound)?;
        if !model.is_manageable() {
            return Err(AppCoreError::CatalogueModelNotFound);
        }
        if self.external_registration(model)?.is_some() {
            return Err(model_runtime::ExternalModelError::Registered.into());
        }
        if model.backend() == InferenceBackend::Ollama {
            return self.install_ollama_catalogue_model(
                model,
                selected_companion_ids,
                cancellation,
                progress,
                installed_at,
            );
        }
        let companions = complete_bundle_companions(model, selected_companion_ids)?;
        let mut artifacts = Vec::with_capacity(companions.len() + 1);
        artifacts.push((model.artifact(), None));
        artifacts.extend(
            companions
                .iter()
                .map(|companion| (companion.artifact(), Some(*companion))),
        );
        let artifact_refs = artifacts
            .iter()
            .map(|(artifact, _)| *artifact)
            .collect::<Vec<_>>();
        self.model_downloads.ensure_disk_space_for(&artifact_refs)?;
        // Installing or replacing any member invalidates the loaded profile as
        // one unit, so an older companion cannot remain resident.
        self.model_runtime.unload_model()?;
        let aggregate_total = checked_artifact_bytes(&artifact_refs)?;
        let mut aggregate_completed = 0_u64;
        let mut verified_artifacts = Vec::with_capacity(artifacts.len());

        for (artifact, companion) in artifacts {
            let base = aggregate_completed;
            let installed =
                self.model_downloads
                    .download(artifact, cancellation, |artifact_progress| {
                        progress(CatalogueInstallProgress {
                            artifact_id: artifact.key().id().to_owned(),
                            companion_kind: companion.map(CompanionArtifact::kind),
                            artifact: artifact_progress,
                            aggregate_downloaded_bytes: base
                                .saturating_add(artifact_progress.downloaded_bytes()),
                            aggregate_total_bytes: aggregate_total,
                        });
                    })?;
            aggregate_completed = aggregate_completed
                .checked_add(artifact.installed_bytes())
                .ok_or(AppCoreError::InvalidCompanionSelection)?;
            verified_artifacts.push((installed, companion));
        }
        if cancellation.is_cancelled() {
            return Err(DownloadError::Cancelled.into());
        }
        let (primary, _) = verified_artifacts
            .first()
            .ok_or(AppCoreError::InvalidCompanionSelection)?;
        let model_metadata = ModelMetadata {
            id: primary.key().to_string(),
            display_name: model.display_name().to_owned(),
            format: model.artifact().format().as_str().to_owned(),
            local_path: primary.path().to_string_lossy().into_owned(),
            content_digest: primary.sha256().to_owned(),
            byte_size: primary.byte_size(),
            imported_at: installed_at.to_owned(),
        };
        let companion_metadata = verified_artifacts
            .iter()
            .skip(1)
            .map(|(installed, companion)| {
                let companion = companion.ok_or(AppCoreError::InvalidCompanionSelection)?;
                Ok(ModelCompanionMetadata {
                    model_id: model_metadata.id.clone(),
                    artifact_id: installed.key().id().to_owned(),
                    kind: companion.kind().as_str().to_owned(),
                    required: companion.required(),
                    local_path: installed.path().to_string_lossy().into_owned(),
                    content_digest: installed.sha256().to_owned(),
                    byte_size: installed.byte_size(),
                    compatibility_version: companion.compatibility_version().to_owned(),
                    memory_requirement_bytes: companion.memory_requirement_bytes(),
                    installed_at: installed_at.to_owned(),
                })
            })
            .collect::<Result<Vec<_>, AppCoreError>>()?;
        self.with_storage(|storage| {
            storage.save_model_bundle(&model_metadata, &companion_metadata)
        })?;
        Ok(CatalogueInstallReceipt {
            installed_bytes: aggregate_completed,
            artifact_ids: verified_artifacts
                .iter()
                .map(|(artifact, _)| artifact.key().id().to_owned())
                .collect(),
        })
    }

    fn install_ollama_catalogue_model(
        &self,
        model: &model_runtime::ModelManifest,
        selected_companion_ids: &[String],
        cancellation: &DownloadCancellation,
        mut progress: impl FnMut(CatalogueInstallProgress),
        installed_at: &str,
    ) -> Result<CatalogueInstallReceipt, AppCoreError> {
        let companions = complete_bundle_companions(model, selected_companion_ids)?;
        let artifacts = std::iter::once(model.artifact())
            .chain(companions.iter().map(|item| item.artifact()))
            .collect::<Vec<_>>();
        let aggregate_total = complete_model_bytes(model)?;
        self.with_ollama_provider(|provider| {
            provider.pull_and_verify(model, cancellation, |snapshot| {
                progress(CatalogueInstallProgress {
                    artifact_id: model.artifact().key().id().to_owned(),
                    companion_kind: None,
                    artifact: snapshot,
                    aggregate_downloaded_bytes: snapshot.downloaded_bytes(),
                    aggregate_total_bytes: aggregate_total,
                });
            })?;
            Ok(())
        })?;
        self.with_storage(|storage| {
            storage.save_provider_model_install(
                &ModelSelection {
                    model_id: model.key().id().to_owned(),
                    model_version: model.key().version().to_owned(),
                },
                aggregate_total,
                installed_at,
            )
        })?;
        Ok(CatalogueInstallReceipt {
            installed_bytes: aggregate_total,
            artifact_ids: artifacts
                .iter()
                .map(|artifact| artifact.key().id().to_owned())
                .collect(),
        })
    }

    fn catalogue_companion_installed(
        &self,
        model: &model_runtime::ModelManifest,
        companion: &CompanionArtifact,
    ) -> Result<bool, AppCoreError> {
        let model_id = model.artifact().key().to_string();
        let artifact_id = companion.artifact().key().id();
        let Some(metadata) =
            self.with_storage(|storage| storage.model_companion(&model_id, artifact_id))?
        else {
            return Ok(false);
        };
        Ok(metadata.kind == companion.kind().as_str()
            && metadata.required == companion.required()
            && metadata.content_digest == companion.artifact().sha256()
            && metadata.byte_size == companion.artifact().installed_bytes()
            && metadata.compatibility_version == companion.compatibility_version()
            && metadata.memory_requirement_bytes == companion.memory_requirement_bytes()
            && self
                .model_downloads
                .is_published(companion.artifact(), Path::new(&metadata.local_path)))
    }

    /// Imports a compatible GGUF into private managed storage and records it.
    ///
    /// The selected source is opened read-only. Only the digest-addressed
    /// managed copy is persisted in the local model catalogue.
    ///
    /// # Errors
    ///
    /// Returns an error when inspection, copying, verification or metadata
    /// persistence fails safely.
    pub fn import_gguf(
        &self,
        source_path: impl AsRef<Path>,
        progress: impl FnMut(ImportProgress),
        imported_at: &str,
    ) -> Result<ImportedModelArtifact, AppCoreError> {
        let imported = self.model_imports.import(source_path, progress)?;
        self.with_storage(|storage| {
            storage.save_model(&ModelMetadata {
                id: format!("imported:{}", imported.sha256()),
                display_name: imported.metadata().name().to_owned(),
                format: "gguf".to_owned(),
                local_path: imported.path().to_string_lossy().into_owned(),
                content_digest: imported.sha256().to_owned(),
                byte_size: imported.byte_size(),
                imported_at: imported_at.to_owned(),
            })
        })?;
        Ok(imported)
    }

    /// Executes one Rust-owned operation against the application store.
    ///
    /// Callers expose only use-case-specific values across IPC; the store and
    /// its `SQLite` connection never cross the frontend boundary.
    ///
    /// # Errors
    ///
    /// Returns an error when the storage lock is unavailable or the operation
    /// reports a persistence failure.
    pub fn with_storage<T>(
        &self,
        operation: impl FnOnce(&mut ApplicationStore) -> Result<T, StorageError>,
    ) -> Result<T, AppCoreError> {
        let mut storage = self
            .storage
            .lock()
            .map_err(|_| AppCoreError::StorageUnavailable)?;
        operation(&mut storage).map_err(Into::into)
    }

    pub(crate) fn with_ollama_provider<T>(
        &self,
        operation: impl FnOnce(&OllamaProvider) -> Result<T, AppCoreError>,
    ) -> Result<T, AppCoreError> {
        let mut service = self
            .ollama_service
            .lock()
            .map_err(|_| AppCoreError::OllamaServiceUnavailable)?;
        operation(service.provider()?)
    }

    pub(crate) fn ensure_glimmer_ollama_model(
        &self,
        model: &model_runtime::ModelManifest,
    ) -> Result<model_runtime::VerifiedOllamaModel, AppCoreError> {
        let external = self.external_primary(model)?;
        let is_external = external.is_some();
        if is_external {
            // The create API touches and can rewrite GGUF source layers. External
            // registrations instead prepare read-only aliases and small metadata.
            self.prepare_external_runtime_references(model)?;
            return self.with_ollama_provider(|provider| {
                provider
                    .verify_registered_tag(GLIMMER_OLLAMA_TAG)
                    .map_err(AppCoreError::from)
            });
        }
        if let Ok(verified) = self.with_ollama_provider(|provider| {
            provider
                .verify_registered_tag(GLIMMER_OLLAMA_TAG)
                .map_err(AppCoreError::from)
        }) {
            return Ok(verified);
        }
        let primary_id = model.artifact().key().to_string();
        let primary = external
            .or(self.with_storage(|storage| storage.model(&primary_id))?)
            .ok_or(AppCoreError::InstalledModelNotFound)?;
        if primary.content_digest != model.artifact().sha256()
            || primary.byte_size != model.artifact().installed_bytes()
            || (!is_external
                && !self
                    .model_downloads
                    .is_published(model.artifact(), Path::new(&primary.local_path)))
        {
            return Err(AppCoreError::InstalledModelInvalid);
        }
        let mut files = BTreeMap::from([(
            model.artifact().file_name().to_owned(),
            format!("sha256:{}", model.artifact().sha256()),
        )]);
        let mut artifacts = vec![(
            Path::new(&primary.local_path).to_owned(),
            model.artifact().sha256().to_owned(),
        )];
        if let Some(profile) = model.multimodal_profile()
            && let Some(id) = profile.projector_artifact_id()
            && let Some(projector) = model.companion(id)
            && let Some(metadata) = self
                .external_companion(model, projector.artifact().key().id())?
                .or(self.with_storage(|storage| {
                    storage.model_companion(&primary_id, projector.artifact().key().id())
                })?)
            && (is_external
                || self
                    .model_downloads
                    .is_published(projector.artifact(), Path::new(&metadata.local_path)))
        {
            files.insert(
                projector.artifact().file_name().to_owned(),
                format!("sha256:{}", projector.artifact().sha256()),
            );
            artifacts.push((
                Path::new(&metadata.local_path).to_owned(),
                projector.artifact().sha256().to_owned(),
            ));
        }
        self.with_ollama_provider(|provider| {
            if !is_external {
                for (path, digest) in &artifacts {
                    self.ensure_ollama_blob(provider, path, digest)?;
                }
            }
            provider
                .create_from_blobs(GLIMMER_OLLAMA_TAG, files)
                .map_err(AppCoreError::from)
        })
    }

    fn ensure_ollama_blob(
        &self,
        provider: &OllamaProvider,
        source: &Path,
        digest: &str,
    ) -> Result<(), AppCoreError> {
        if let Some(models_directory) = &self.ollama_models_directory {
            let blobs = models_directory.join("blobs");
            fs::create_dir_all(&blobs)?;
            restrict_directory_permissions(&blobs)?;
            let target = blobs.join(format!("sha256-{digest}"));
            if !target.exists() {
                match fs::hard_link(source, &target) {
                    Ok(()) => {}
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::CrossesDevices
                                | std::io::ErrorKind::PermissionDenied
                                | std::io::ErrorKind::Unsupported
                        ) => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
        provider.ensure_blob(source, digest).map_err(Into::into)
    }

    #[cfg(test)]
    pub(crate) fn stop_managed_ollama_for_test(&self) -> Result<bool, AppCoreError> {
        self.ollama_shutdown.stop_managed()
    }

    #[cfg(test)]
    pub(crate) fn managed_ollama_status_for_test(
        &self,
    ) -> Result<Option<model_runtime::OllamaSidecarStatus>, AppCoreError> {
        self.ollama_shutdown.managed_status()
    }
}

pub(super) fn selected_companions<'a>(
    model: &'a model_runtime::ModelManifest,
    selected_ids: &[String],
) -> Result<Vec<&'a CompanionArtifact>, AppCoreError> {
    let mut unique = std::collections::HashSet::with_capacity(selected_ids.len());
    for selected in selected_ids {
        if !unique.insert(selected.as_str()) || model.companion(selected).is_none() {
            return Err(AppCoreError::InvalidCompanionSelection);
        }
    }
    Ok(model
        .companions()
        .iter()
        .filter(|companion| {
            companion.required() || unique.contains(companion.artifact().key().id())
        })
        .collect())
}

pub(crate) fn complete_bundle_companions<'a>(
    model: &'a model_runtime::ModelManifest,
    legacy_selected_ids: &[String],
) -> Result<Vec<&'a CompanionArtifact>, AppCoreError> {
    // Retain validation for older IPC clients, but installation policy is now
    // catalogue-owned and always resolves the complete declared bundle.
    selected_companions(model, legacy_selected_ids)?;
    Ok(model.companions().iter().collect())
}
pub(super) fn checked_artifact_bytes(artifacts: &[&ModelArtifact]) -> Result<u64, AppCoreError> {
    artifacts.iter().try_fold(0_u64, |total, artifact| {
        total
            .checked_add(artifact.installed_bytes())
            .ok_or(AppCoreError::InvalidCompanionSelection)
    })
}

pub(super) fn complete_model_bytes(
    model: &model_runtime::ModelManifest,
) -> Result<u64, AppCoreError> {
    if let Some(bytes) = model
        .ollama_distribution()
        .and_then(model_runtime::OllamaDistribution::tensor_bundle_bytes)
    {
        return Ok(bytes);
    }
    let artifacts = std::iter::once(model.artifact())
        .chain(model.companions().iter().map(CompanionArtifact::artifact))
        .collect::<Vec<_>>();
    checked_artifact_bytes(&artifacts)
}

pub(crate) fn resolve_new_conversation_selection(
    configured: Option<&ModelSelection>,
    candidates: &[(ModelSelection, bool)],
) -> Option<ModelSelection> {
    if let Some(configured) = configured
        && candidates
            .iter()
            .any(|(candidate, _)| candidate == configured)
    {
        // Explicit choices remain stable through provider outages or removal;
        // generation reports unavailability instead of silently falling back.
        return Some(configured.clone());
    }
    let installed = candidates
        .iter()
        .filter(|(_, installed)| *installed)
        .collect::<Vec<_>>();
    match installed.as_slice() {
        [] => None,
        [only] => Some(only.0.clone()),
        many => model_runtime::ModelPlatform::current()
            .preferred_model_ids()
            .iter()
            .find_map(|id| {
                many.iter()
                    .find(|(selection, _)| selection.model_id == *id)
                    .map(|(selection, _)| selection.clone())
            }),
    }
}
pub(crate) fn validate_inference_limits(
    context_tokens: u32,
    output_tokens: u32,
) -> Result<(), AppCoreError> {
    if context_tokens == 0
        || context_tokens > MAX_PRODUCT_CONTEXT_TOKENS
        || output_tokens == 0
        || output_tokens > MAX_PRODUCT_OUTPUT_TOKENS
        || output_tokens > context_tokens
    {
        return Err(AppCoreError::InvalidInferenceLimits);
    }
    Ok(())
}
