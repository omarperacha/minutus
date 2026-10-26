use super::{
    ArtifactKey, CATALOGUE_SCHEMA_VERSION, CatalogueError, CompanionArtifact, HashSet,
    MAX_CATALOGUE_BYTES, MAX_MODELS, ModelArtifact, ModelKey, ModelManifest, RawCatalogue,
    validate_model,
};

/// Validated, immutable catalogue owned by Rust.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelCatalogue {
    schema_version: u32,
    models: Vec<ModelManifest>,
}

impl ModelCatalogue {
    /// Parses untrusted JSON into validated runtime values.
    ///
    /// # Errors
    ///
    /// Returns a typed error for malformed, incompatible, incomplete, duplicate
    /// or semantically invalid catalogue data.
    pub fn parse_json(json: &str) -> Result<Self, CatalogueError> {
        if json.len() > MAX_CATALOGUE_BYTES {
            return Err(CatalogueError::TooLarge);
        }
        let raw: RawCatalogue = serde_json::from_str(json).map_err(CatalogueError::Json)?;
        if raw.schema_version != CATALOGUE_SCHEMA_VERSION {
            return Err(CatalogueError::UnsupportedSchema {
                found: raw.schema_version,
                supported: CATALOGUE_SCHEMA_VERSION,
            });
        }
        if raw.models.is_empty() {
            return Err(CatalogueError::NoModels);
        }
        if raw.models.len() > MAX_MODELS {
            return Err(CatalogueError::TooManyModels);
        }

        let mut model_keys = HashSet::with_capacity(raw.models.len());
        let mut artifact_keys = HashSet::with_capacity(raw.models.len());
        let mut models = Vec::with_capacity(raw.models.len());
        for (index, model) in raw.models.into_iter().enumerate() {
            let validated = validate_model(model, index)?;
            if !model_keys.insert(validated.key.clone()) {
                return Err(CatalogueError::DuplicateModel(validated.key));
            }
            for artifact in std::iter::once(&validated.artifact)
                .chain(validated.companions.iter().map(CompanionArtifact::artifact))
            {
                if !artifact_keys.insert(artifact.key.clone()) {
                    return Err(CatalogueError::DuplicateArtifact(artifact.key.clone()));
                }
            }
            models.push(validated);
        }
        Ok(Self {
            schema_version: raw.schema_version,
            models,
        })
    }

    /// Returns the validated schema version.
    #[must_use]
    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Returns all validated releases in display order.
    #[must_use]
    pub fn models(&self) -> &[ModelManifest] {
        &self.models
    }

    /// Resolves one exact model release.
    #[must_use]
    pub fn model(&self, key: &ModelKey) -> Option<&ModelManifest> {
        self.models.iter().find(|model| model.key() == key)
    }

    /// Resolves one exact artifact.
    #[must_use]
    pub fn artifact(&self, key: &ArtifactKey) -> Option<&ModelArtifact> {
        self.models
            .iter()
            .flat_map(|model| {
                std::iter::once(model.artifact())
                    .chain(model.companions().iter().map(CompanionArtifact::artifact))
            })
            .find(|artifact| artifact.key() == key)
    }
}
