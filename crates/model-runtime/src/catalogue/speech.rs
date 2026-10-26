use serde::Deserialize;

use super::{ArtifactFormat, CatalogueError, ModelArtifact, ModelKey, validation};

/// Recognition engines, independent of audio devices and chat providers.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum SpeechBackend {
    /// Portable native Whisper runtime.
    WhisperCpp,
}

/// Immutable ASR profile. It cannot be passed to chat-profile APIs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpeechProfile {
    key: ModelKey,
    display_name: String,
    artifact: ModelArtifact,
    backend: SpeechBackend,
    runtime_revision: String,
    license_url: url::Url,
}

impl SpeechProfile {
    /// Exact model release.
    #[must_use]
    pub const fn key(&self) -> &ModelKey {
        &self.key
    }
    /// Human-readable profile label.
    #[must_use]
    pub fn display_name(&self) -> &str {
        &self.display_name
    }
    /// Verified-download specification.
    #[must_use]
    pub const fn artifact(&self) -> &ModelArtifact {
        &self.artifact
    }
    /// Recognition backend, without platform-specific acceleration policy.
    #[must_use]
    pub const fn backend(&self) -> SpeechBackend {
        self.backend
    }
    /// Required native runtime source revision.
    #[must_use]
    pub fn runtime_revision(&self) -> &str {
        &self.runtime_revision
    }
    /// Publisher licence.
    #[must_use]
    pub fn license_url(&self) -> &url::Url {
        &self.license_url
    }
}

/// Speech-only catalogue, versioned independently of chat profiles.
#[derive(Clone, Debug)]
pub struct SpeechCatalogue {
    profiles: Vec<SpeechProfile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawCatalogue {
    schema_version: u32,
    profiles: Vec<RawProfile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawProfile {
    id: String,
    version: String,
    display_name: String,
    artifact: validation::RawModelArtifact,
    backend: SpeechBackend,
    runtime_revision: String,
    license_url: String,
}

impl SpeechCatalogue {
    /// Validates a bounded speech catalogue without relaxing chat validation.
    ///
    /// # Errors
    /// Rejects unknown schemas, fields, formats, duplicate identities and invalid metadata.
    pub fn parse_json(json: &str) -> Result<Self, CatalogueError> {
        if json.len() > super::MAX_CATALOGUE_BYTES {
            return Err(CatalogueError::TooLarge);
        }
        let raw: RawCatalogue = serde_json::from_str(json).map_err(CatalogueError::Json)?;
        if raw.schema_version != 1 {
            return Err(CatalogueError::UnsupportedSchema {
                found: raw.schema_version,
                supported: 1,
            });
        }
        if raw.profiles.is_empty() {
            return Err(CatalogueError::NoModels);
        }
        if raw.profiles.len() > super::MAX_MODELS {
            return Err(CatalogueError::TooManyModels);
        }
        let mut profiles = Vec::new();
        let mut keys = std::collections::HashSet::new();
        for (index, raw) in raw.profiles.into_iter().enumerate() {
            let invalid = |field, reason| validation::invalid(index, field, reason);
            validation::validate_identifier(&raw.id, super::MAX_ID_BYTES)
                .map_err(|e| invalid("id", e))?;
            validation::validate_version_component(&raw.version, super::MAX_VERSION_BYTES)
                .map_err(|e| invalid("version", e))?;
            validation::validate_text(&raw.display_name, super::MAX_DISPLAY_NAME_BYTES)
                .map_err(|e| invalid("displayName", e))?;
            validation::validate_version_component(&raw.runtime_revision, super::MAX_VERSION_BYTES)
                .map_err(|e| invalid("runtimeRevision", e))?;
            let key = ModelKey {
                id: raw.id,
                version: raw.version,
            };
            if !keys.insert(key.clone()) {
                return Err(CatalogueError::DuplicateModel(key));
            }
            let artifact = validation::validate_artifact(raw.artifact, &key, index)?;
            if artifact.format() != ArtifactFormat::WhisperGgml
                || artifact.download_bytes() != artifact.installed_bytes()
            {
                return Err(invalid(
                    "artifact",
                    "Whisper requires directly installed whisper-ggml weights",
                ));
            }
            let license_url = validation::validate_https_url(&raw.license_url)
                .map_err(|e| invalid("licenseUrl", e))?;
            profiles.push(SpeechProfile {
                key,
                display_name: raw.display_name,
                artifact,
                backend: raw.backend,
                runtime_revision: raw.runtime_revision,
                license_url,
            });
        }
        Ok(Self { profiles })
    }

    /// Speech releases in display order; never included in the chat catalogue.
    #[must_use]
    pub fn profiles(&self) -> &[SpeechProfile] {
        &self.profiles
    }
}

/// Returns the ASR-001 artifact pins bundled in this build.
///
/// # Errors
/// Returns an error if the bundled catalogue violates its contract.
pub fn bundled_speech_catalogue() -> Result<SpeechCatalogue, CatalogueError> {
    SpeechCatalogue::parse_json(include_str!("../../catalogue/speech-v1.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn bundled_profile_matches_qualification_and_is_absent_from_chat()
    -> Result<(), Box<dyn std::error::Error>> {
        let catalogue = bundled_speech_catalogue()?;
        let profile = &catalogue.profiles()[0];
        let lock: Value =
            serde_json::from_str(include_str!("../../../../scripts/asr/models.lock.json"))?;
        assert_eq!(
            profile.artifact().sha256(),
            lock["artifacts"][3]["sha256"]
                .as_str()
                .ok_or("missing digest")?
        );
        assert_eq!(
            profile.artifact().download_bytes(),
            lock["artifacts"][3]["bytes"]
                .as_u64()
                .ok_or("missing size")?
        );
        assert_eq!(
            profile.artifact().download_url().as_str(),
            lock["artifacts"][3]["download_url"]
                .as_str()
                .ok_or("missing URL")?
        );
        assert_eq!(
            profile.runtime_revision(),
            crate::asr::WHISPER_RUNTIME_REVISION
        );
        assert!(crate::bundled_catalogue()?.model(profile.key()).is_none());
        Ok(())
    }

    #[test]
    fn speech_and_chat_catalogues_reject_cross_task_formats_and_unknown_fields()
    -> Result<(), Box<dyn std::error::Error>> {
        let valid: Value = serde_json::from_str(include_str!("../../catalogue/speech-v1.json"))?;
        for (field, value) in [
            ("format", "gguf"),
            ("format", "coreml"),
            ("sha256", "bad"),
            ("fileName", "../weights.bin"),
            ("downloadUrl", "http://example.test/weights"),
        ] {
            let mut doc = valid.clone();
            doc["profiles"][0]["artifact"][field] = value.into();
            assert!(SpeechCatalogue::parse_json(&doc.to_string()).is_err());
        }
        let mut doc = valid.clone();
        doc["profiles"][0]["captureDevice"] = "microphone".into();
        assert!(SpeechCatalogue::parse_json(&doc.to_string()).is_err());
        let mut chat: Value = serde_json::from_str(include_str!("../../catalogue/v4.json"))?;
        chat["models"][0]["artifact"]["format"] = "whisper-ggml".into();
        assert!(crate::ModelCatalogue::parse_json(&chat.to_string()).is_err());
        let mut duplicate = valid.clone();
        duplicate["profiles"]
            .as_array_mut()
            .ok_or("missing profiles")?
            .push(valid["profiles"][0].clone());
        assert!(SpeechCatalogue::parse_json(&duplicate.to_string()).is_err());
        Ok(())
    }
}
