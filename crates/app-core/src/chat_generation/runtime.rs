use super::*;

impl AppCore {
    pub(crate) fn model_for_binding(
        &self,
        binding: &GenerationBinding,
    ) -> Result<&model_runtime::ModelManifest, AppCoreError> {
        self.model_catalogue
            .models()
            .iter()
            .find(|model| {
                model.key().id() == binding.model_id()
                    && model.key().version() == binding.model_version()
                    && model.artifact().sha256() == binding.model_sha256()
            })
            .ok_or(AppCoreError::CatalogueModelNotFound)
    }

    /// Returns the safe identity of the exact system instructions for a run.
    ///
    /// Persisted agent-run state can retain this value without storing or
    /// exposing complete prompt text.
    ///
    /// # Errors
    ///
    /// Returns an error when trusted instruction assembly fails closed.
    pub fn agent_system_instruction_identity(
        &self,
        conversation: &Conversation,
        current_date: &str,
    ) -> Result<AgentSystemInstructionIdentity, AppCoreError> {
        Ok(
            build_agent_system_instructions(
                current_date,
                latest_user_selection_text(conversation),
            )?
            .identity()
            .clone(),
        )
    }

    /// Loads or recovers the first installed catalogue model for chat generation.
    ///
    /// # Errors
    ///
    /// Returns an error when no verified catalogue model is installed or lifecycle
    /// recovery and loading fail.
    pub fn ensure_chat_model_loaded(
        &self,
        executable: impl AsRef<Path>,
    ) -> Result<u32, AppCoreError> {
        self.ensure_chat_model_loaded_for_assets(executable, false)
    }

    /// Loads or recovers the installed chat model with vision when required.
    ///
    /// # Errors
    ///
    /// Returns an error when the primary model or required verified vision
    /// projector is unavailable, or model lifecycle recovery fails.
    pub fn ensure_chat_model_loaded_for_assets(
        &self,
        executable: impl AsRef<Path>,
        require_multimodal: bool,
    ) -> Result<u32, AppCoreError> {
        let selection = self
            .new_default_conversation_model()?
            .ok_or(AppCoreError::InstalledModelNotFound)?;
        self.ensure_selected_chat_model_loaded_for_assets(
            executable,
            &selection,
            require_multimodal,
        )
    }

    /// Loads the exact conversation-bound release without trying another model.
    ///
    /// # Errors
    ///
    /// Returns an error when the selected release, provider, or required profile is unavailable.
    pub fn ensure_selected_chat_model_loaded_for_assets(
        &self,
        executable: impl AsRef<Path>,
        selection: &ModelSelection,
        require_multimodal: bool,
    ) -> Result<u32, AppCoreError> {
        let model = self.validated_selection(selection)?;
        self.prepare_external_runtime_references(model)?;
        if !model.is_integrated() {
            return Err(AppCoreError::CatalogueModelNotFound);
        }
        if model.key().id() == "meta-models/muse-glimmer-30b" {
            self.ensure_glimmer_ollama_model(model)?;
            if require_multimodal {
                let profile = model
                    .multimodal_profile()
                    .ok_or(AppCoreError::VisionProjectorNotInstalled)?;
                if !self.catalogue_companion_installed_by_id(
                    model.key(),
                    profile
                        .projector_artifact_id()
                        .ok_or(AppCoreError::VisionProjectorNotInstalled)?,
                )? {
                    return Err(AppCoreError::VisionProjectorNotInstalled);
                }
            }
            return Ok(product_output_tokens(model));
        }
        if model.backend() == InferenceBackend::Ollama {
            if require_multimodal {
                let profile = model
                    .multimodal_profile()
                    .ok_or(AppCoreError::VisionProjectorNotInstalled)?;
                if let Some(projector) = profile.projector_artifact_id()
                    && !self.catalogue_companion_installed_by_id(model.key(), projector)?
                {
                    return Err(AppCoreError::VisionProjectorNotInstalled);
                }
            }
            self.with_ollama_provider(|provider| {
                provider.verify_model(model).map_err(AppCoreError::from)
            })?;
            return Ok(product_output_tokens(model));
        }
        let loaded_matches = self
            .model_runtime
            .active_profile()
            .ok()
            .is_some_and(|profile| {
                profile.binding().model_id() == selection.model_id
                    && profile.binding().model_version() == selection.model_version
            });
        match self.model_runtime.status()? {
            InferenceRuntimeStatus::Loaded {
                output_tokens,
                multimodal,
                ..
            } if loaded_matches && (!require_multimodal || multimodal) => return Ok(output_tokens),
            InferenceRuntimeStatus::Loaded { .. } | InferenceRuntimeStatus::Unloaded => {}
            InferenceRuntimeStatus::Generating { .. } => {
                return Err(InferenceError::GenerationAlreadyActive.into());
            }
            InferenceRuntimeStatus::Failed { .. } => {
                self.model_runtime.restart_after_failure()?;
                if let InferenceRuntimeStatus::Loaded {
                    output_tokens,
                    multimodal,
                    ..
                } = self.model_runtime.status()?
                    && loaded_matches
                    && (!require_multimodal || multimodal)
                {
                    return Ok(output_tokens);
                }
            }
        }

        let key = model.key();
        let defaults = model.defaults();
        let output_tokens = product_output_tokens(model);
        let selected_companion_ids = if require_multimodal {
            let model = self
                .model_catalogue
                .model(key)
                .ok_or(AppCoreError::CatalogueModelNotFound)?;
            let projector_id = model
                .multimodal_profile()
                .ok_or(AppCoreError::VisionProjectorNotInstalled)?
                .projector_artifact_id()
                .ok_or(AppCoreError::VisionProjectorNotInstalled)?;
            if !self.catalogue_companion_installed_by_id(key, projector_id)? {
                return Err(AppCoreError::VisionProjectorNotInstalled);
            }
            vec![projector_id.to_owned()]
        } else {
            Vec::new()
        };
        self.load_catalogue_model_with_companions(
            executable,
            key,
            defaults.context_tokens(),
            output_tokens,
            &selected_companion_ids,
        )?;
        Ok(output_tokens)
    }

    /// Stops the matching active local generation.
    ///
    /// # Errors
    ///
    /// Returns an error when runtime cancellation fails.
    pub fn stop_generation(&self, generation_id: &str) -> Result<bool, AppCoreError> {
        if let Some(cancellation) = self
            .ollama_generations
            .lock()
            .map_err(|_| AppCoreError::OllamaServiceUnavailable)?
            .get(generation_id)
            .cloned()
        {
            cancellation.cancel();
            return Ok(true);
        }
        self.model_runtime
            .stop_generation(generation_id)
            .map_err(Into::into)
    }

    /// Loads digest-verified normalized bytes for local inference by opaque ID.
    ///
    /// # Errors
    ///
    /// Returns an error when metadata or managed bytes are missing or invalid.
    pub fn inference_asset(&self, asset_id: &AssetId) -> Result<Vec<u8>, AppCoreError> {
        let _operation = self
            .asset_operations
            .lock()
            .map_err(|_| AppCoreError::AssetOperationsUnavailable)?;
        let asset = self
            .with_storage(|storage| storage.asset(asset_id))?
            .ok_or(AppCoreError::AssetNotFound)?;
        self.asset_files.read_inference(&asset).map_err(Into::into)
    }

    /// Persists a bounded untrusted extraction for rich inputs submitted to one
    /// successfully completed model round.
    ///
    /// The checkpoint is bound to exact history, managed source digests and the
    /// immutable model/adapter/provider binding. Invalid or empty extraction is
    /// rejected without replacing the last valid extraction. Byte presentation
    /// is governed independently by the request-local presentation ledger.
    ///
    /// # Errors
    ///
    /// Returns an error when source verification, history encoding or persistence fails.
    pub fn capture_rich_evidence_checkpoint(
        &self,
        conversation: &Conversation,
        asset_ids: &[AssetId],
        binding_sha256: &str,
        extraction: &str,
        updated_at: &str,
    ) -> Result<bool, AppCoreError> {
        working_context::rich_evidence::capture_checkpoint(
            self,
            conversation,
            asset_ids,
            binding_sha256,
            extraction,
            updated_at,
        )
    }

    /// Streams one generation with only digest-verified managed image derivatives.
    ///
    /// The caller supplies opaque asset IDs, never bytes, paths or URLs. Rust
    /// resolves each committed asset while holding the asset-operation lock and
    /// the runtime applies the selected profile's image and context limits.
    ///
    /// # Errors
    ///
    /// Returns an error if an asset is absent or changed, the image/profile
    /// limits fail, memory is pressured, or local inference fails.
    #[expect(
        clippy::too_many_lines,
        reason = "asset verification and both adapter-specific generation paths remain one lock-scoped operation"
    )]
    #[expect(
        clippy::too_many_arguments,
        reason = "the trusted runtime boundary binds both context and output ceilings to assets and model identity"
    )]
    pub fn start_generation_with_assets(
        &self,
        generation_id: &str,
        prompt: PreparedChatPrompt,
        context_tokens: u32,
        output_tokens: u32,
        binding: &GenerationBinding,
        asset_ids: &[AssetId],
        emit: impl FnMut(GenerationEvent),
    ) -> Result<(), AppCoreError> {
        if asset_ids.len() > MAX_GENERATION_IMAGES {
            return Err(InferenceError::ImageCountExceeded.into());
        }
        let asset_operation = self
            .asset_operations
            .lock()
            .map_err(|_| AppCoreError::AssetOperationsUnavailable)?;
        let images = asset_ids
            .iter()
            .map(|asset_id| {
                let asset = self
                    .with_storage(|storage| storage.asset(asset_id))?
                    .ok_or(AppCoreError::AssetNotFound)?;
                self.asset_files
                    .read_inference(&asset)
                    .map_err(AppCoreError::from)
            })
            .collect::<Result<Vec<_>, AppCoreError>>()?;
        drop(asset_operation);
        match prompt {
            PreparedChatPrompt::Local { rendered, tools } => {
                let model = self
                    .model_catalogue
                    .models()
                    .iter()
                    .find(|model| {
                        model.key().id() == binding.model_id()
                            && model.key().version() == binding.model_version()
                    })
                    .ok_or(AppCoreError::CatalogueModelNotFound)?;
                let verified = self.ensure_glimmer_ollama_model(model)?;
                if context_tokens < output_tokens
                    || context_tokens > model.capabilities().context_limit_tokens()
                    || context_tokens > crate::MAX_PRODUCT_CONTEXT_TOKENS
                {
                    return Err(InferenceError::ProfileBindingMismatch.into());
                }
                let expected = GenerationBinding::glimmer_ollama(
                    model.key().id(),
                    model.key().version(),
                    model.artifact().sha256(),
                    verified.provider_version(),
                    model.capabilities().clone(),
                );
                if *binding != expected || output_tokens > MAX_PRODUCT_OUTPUT_TOKENS {
                    return Err(InferenceError::ProfileBindingMismatch.into());
                }
                let prompt = glimmer_ollama_image_markers(rendered, images.len())?;
                let encoded_images = images
                    .iter()
                    .map(|image| BASE64_STANDARD.encode(image))
                    .collect();
                let request = model_runtime::OllamaChatRequest::adapter_prompt(
                    &verified,
                    prompt,
                    encoded_images,
                    &tools,
                )
                .with_context(context_tokens, output_tokens);
                let mut decoder = QwenStreamDecoder::new(generation_id)
                    .map_err(|_| InferenceError::InvalidPrompt)?;
                let cancellation = DownloadCancellation::new();
                if self
                    .ollama_generations
                    .lock()
                    .map_err(|_| AppCoreError::OllamaServiceUnavailable)?
                    .insert(generation_id.to_owned(), cancellation.clone())
                    .is_some()
                {
                    return Err(InferenceError::GenerationAlreadyActive.into());
                }
                let result = self.with_ollama_provider(|provider| {
                    provider
                        .stream_chat_events(&request, &cancellation, &mut decoder, emit)
                        .map_err(AppCoreError::from)
                });
                self.ollama_generations
                    .lock()
                    .map_err(|_| AppCoreError::OllamaServiceUnavailable)?
                    .remove(generation_id);
                result
            }
            PreparedChatPrompt::Qwen(request) => {
                let model = self
                    .model_catalogue
                    .models()
                    .iter()
                    .find(|model| {
                        model.key().id() == binding.model_id()
                            && model.key().version() == binding.model_version()
                    })
                    .ok_or(AppCoreError::CatalogueModelNotFound)?;
                if output_tokens > MAX_PRODUCT_OUTPUT_TOKENS {
                    return Err(InferenceError::ProfileBindingMismatch.into());
                }
                if context_tokens < output_tokens
                    || context_tokens > model.capabilities().context_limit_tokens()
                    || context_tokens > crate::MAX_PRODUCT_CONTEXT_TOKENS
                {
                    return Err(InferenceError::ProfileBindingMismatch.into());
                }
                let mut decoder = QwenStreamDecoder::new(generation_id)
                    .map_err(|_| InferenceError::InvalidPrompt)?;
                let cancellation = DownloadCancellation::new();
                if self
                    .ollama_generations
                    .lock()
                    .map_err(|_| AppCoreError::OllamaServiceUnavailable)?
                    .insert(generation_id.to_owned(), cancellation.clone())
                    .is_some()
                {
                    return Err(InferenceError::GenerationAlreadyActive.into());
                }
                let result = self.with_ollama_provider(|provider| {
                    let verified = provider.verify_model(model)?;
                    let expected = GenerationBinding::qwen_ollama(
                        model.key().id(),
                        model.key().version(),
                        model.artifact().sha256(),
                        verified.provider_version(),
                        model.capabilities().clone(),
                    )
                    .with_qwen_runtime_profile(verified.runtime_profile(context_tokens));
                    if *binding != expected {
                        return Err(InferenceError::ProfileBindingMismatch.into());
                    }
                    let tuning = binding
                        .qwen_runtime_profile()
                        .ok_or(InferenceError::ProfileBindingMismatch)?;
                    let _ = self.record_diagnostic(
                        audit_log::DiagnosticEvent::InferenceProfileSelected {
                            profile: if tuning.is_measured_baseline() {
                                audit_log::InferenceTuningProfile::QwenM1ProMtp4V1
                            } else {
                                audit_log::InferenceTuningProfile::QwenExistingV1
                            },
                            context_tokens: tuning.context_tokens(),
                        },
                    );
                    let native = QwenAdapter.render_with_context(
                        &verified,
                        &request,
                        &images,
                        context_tokens,
                        output_tokens,
                    )?;
                    provider
                        .stream_chat_events(&native, &cancellation, &mut decoder, emit)
                        .map_err(AppCoreError::from)
                });
                self.ollama_generations
                    .lock()
                    .map_err(|_| AppCoreError::OllamaServiceUnavailable)?
                    .remove(generation_id);
                result
            }
        }
    }
}
