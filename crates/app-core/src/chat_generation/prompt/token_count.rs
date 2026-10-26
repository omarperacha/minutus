//! Selects the verified vocabulary representation without exposing backend details.

use model_runtime::{InferenceBackend, ModelManifest};

use crate::{AppCore, AppCoreError};

impl AppCore {
    pub(super) fn count_model_text_tokens(
        &self,
        model: &ModelManifest,
        text: &str,
    ) -> Result<u64, AppCoreError> {
        if let Some(metadata) = self.external_primary(model)? {
            return self
                .exact_tokenizer
                .count(metadata.local_path, text)
                .map_err(Into::into);
        }
        if model.backend() == InferenceBackend::Ollama {
            let blobs = self
                .ollama_models_directory
                .as_ref()
                .ok_or(AppCoreError::InstalledModelInvalid)?
                .join("blobs");
            if let Some(tokenizer) = model
                .ollama_distribution()
                .and_then(model_runtime::OllamaDistribution::tokenizer_blob)
            {
                return self
                    .exact_tokenizer
                    .count_ollama_tensor(
                        blobs.join(format!("sha256-{}", tokenizer.sha256())),
                        tokenizer.sha256(),
                        text,
                    )
                    .map_err(Into::into);
            }
            return self
                .exact_tokenizer
                .count(
                    blobs.join(format!("sha256-{}", model.artifact().sha256())),
                    text,
                )
                .map_err(Into::into);
        }
        let path = self
            .with_storage(|storage| storage.model(&model.artifact().key().to_string()))?
            .ok_or(AppCoreError::InstalledModelNotFound)?
            .local_path;
        self.exact_tokenizer.count(path, text).map_err(Into::into)
    }
}
