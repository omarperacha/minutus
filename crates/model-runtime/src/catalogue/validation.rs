use super::{
    ArtifactFormat, ArtifactKey, CatalogueError, CompanionArtifact, CompanionKind, Deserialize,
    HashSet, InferenceBackend, MAX_COMPANIONS, MAX_CONTEXT_TOKENS, MAX_DISPLAY_NAME_BYTES,
    MAX_FILENAME_BYTES, MAX_ID_BYTES, MAX_PROFILE_IMAGES, MAX_VERSION_BYTES, ModelArtifact,
    ModelCapabilities, ModelCapability, ModelDefaults, ModelIntegrationStatus, ModelKey,
    ModelManifest, MultimodalProfile, OllamaDistribution, Url,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct RawCatalogue {
    pub(super) schema_version: u32,
    pub(super) models: Vec<RawModelManifest>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct RawModelManifest {
    id: String,
    version: String,
    display_name: String,
    artifact: RawModelArtifact,
    companions: Vec<RawCompanionArtifact>,
    companion_compatibility_version: String,
    multimodal_profile: Option<RawMultimodalProfile>,
    recommended_memory_bytes: u64,
    defaults: RawModelDefaults,
    integration_status: ModelIntegrationStatus,
    capabilities: RawModelCapabilities,
    backend: RawBackend,
    license_url: String,
    usage_policy_url: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct RawModelArtifact {
    id: String,
    file_name: String,
    format: ArtifactFormat,
    download_url: String,
    download_bytes: u64,
    installed_bytes: u64,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawCompanionArtifact {
    #[serde(flatten)]
    artifact: RawModelArtifact,
    kind: CompanionKind,
    required: bool,
    compatibility_version: String,
    memory_requirement_bytes: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawMultimodalProfile {
    projector_artifact_id: Option<String>,
    image_tokens: u32,
    max_images: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawModelDefaults {
    context_tokens: u32,
    output_tokens: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawModelCapabilities {
    features: Vec<ModelCapability>,
    context_limit_tokens: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawBackend {
    kind: InferenceBackend,
    compatibility_version: String,
    ollama: Option<RawOllamaDistribution>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawOllamaDistribution {
    tensor_bundle_manifest: Option<String>,
    tag: String,
    manifest_sha256: String,
    config_sha256: String,
    config_bytes: u64,
    parameters_sha256: String,
    parameters_bytes: u64,
    license_sha256: String,
    license_bytes: u64,
    renderer: String,
    parser: String,
}

pub(super) fn validate_model(
    raw: RawModelManifest,
    index: usize,
) -> Result<ModelManifest, CatalogueError> {
    validate_identifier(&raw.id, MAX_ID_BYTES).map_err(|reason| invalid(index, "id", reason))?;
    validate_version_component(&raw.version, MAX_VERSION_BYTES)
        .map_err(|reason| invalid(index, "version", reason))?;
    validate_text(&raw.display_name, MAX_DISPLAY_NAME_BYTES)
        .map_err(|reason| invalid(index, "displayName", reason))?;
    let key = ModelKey {
        id: raw.id,
        version: raw.version,
    };
    let artifact = validate_artifact(raw.artifact, &key, index)?;
    if raw.companions.len() > MAX_COMPANIONS {
        return Err(invalid(
            index,
            "companions",
            "contains too many companion artifacts",
        ));
    }
    validate_version_component(&raw.companion_compatibility_version, MAX_VERSION_BYTES)
        .map_err(|reason| invalid(index, "companionCompatibilityVersion", reason))?;
    let mut companion_ids = HashSet::with_capacity(raw.companions.len());
    let mut companion_kinds = HashSet::with_capacity(raw.companions.len());
    let mut companions = Vec::with_capacity(raw.companions.len());
    for (companion_index, companion) in raw.companions.into_iter().enumerate() {
        let field = match companion_index {
            0 => "companions[0]",
            1 => "companions[1]",
            _ => "companions[n]",
        };
        let validated = validate_companion(
            companion,
            &key,
            &raw.companion_compatibility_version,
            index,
            field,
        )?;
        if validated.artifact.key.id == artifact.key.id
            || !companion_ids.insert(validated.artifact.key.id.clone())
        {
            return Err(invalid(
                index,
                field,
                "artifact id must be unique within the model",
            ));
        }
        if !companion_kinds.insert(validated.kind) {
            return Err(invalid(
                index,
                field,
                "companion kind must be unique within the model",
            ));
        }
        companions.push(validated);
    }
    if raw.recommended_memory_bytes < artifact.installed_bytes {
        return Err(invalid(
            index,
            "recommendedMemoryBytes",
            "must cover the installed primary artifact",
        ));
    }
    let defaults = validate_defaults(&raw.defaults, index)?;
    let capabilities = validate_capabilities(raw.capabilities, defaults, index)?;
    validate_version_component(&raw.backend.compatibility_version, MAX_VERSION_BYTES)
        .map_err(|reason| invalid(index, "backend.compatibilityVersion", reason))?;
    let ollama = validate_ollama_distribution(raw.backend.kind, raw.backend.ollama, index)?;
    let embedded_vision = validate_tensor_artifact(&artifact, &companions, ollama.as_ref(), index)?;
    let multimodal_profile = validate_multimodal_profile(
        raw.multimodal_profile,
        &companions,
        defaults,
        embedded_vision,
        index,
    )?;
    let license_url = validate_https_url(&raw.license_url)
        .map_err(|reason| invalid(index, "licenseUrl", reason))?;
    let usage_policy_url = validate_https_url(&raw.usage_policy_url)
        .map_err(|reason| invalid(index, "usagePolicyUrl", reason))?;

    Ok(ModelManifest {
        key,
        display_name: raw.display_name,
        artifact,
        companions,
        companion_compatibility_version: raw.companion_compatibility_version,
        multimodal_profile,
        recommended_memory_bytes: raw.recommended_memory_bytes,
        defaults,
        integration_status: raw.integration_status,
        capabilities,
        backend: raw.backend.kind,
        backend_compatibility_version: raw.backend.compatibility_version,
        ollama,
        license_url,
        usage_policy_url,
    })
}

fn validate_tensor_artifact(
    artifact: &ModelArtifact,
    companions: &[CompanionArtifact],
    ollama: Option<&OllamaDistribution>,
    index: usize,
) -> Result<bool, CatalogueError> {
    if !matches!(
        artifact.format(),
        ArtifactFormat::Gguf | ArtifactFormat::Safetensors
    ) {
        return Err(invalid(
            index,
            "artifact.format",
            "chat profiles require GGUF or provider safetensors",
        ));
    }
    let bundle = ollama.and_then(|d| d.tensor_bundle.as_ref());
    if (artifact.format() == ArtifactFormat::Safetensors) != bundle.is_some()
        || bundle.is_some_and(|b| !b.contains_artifact(artifact) || !companions.is_empty())
    {
        return Err(invalid(
            index,
            "artifact.format",
            "safetensors require a complete provider bundle",
        ));
    }
    Ok(bundle.is_some())
}

fn validate_defaults(
    raw: &RawModelDefaults,
    index: usize,
) -> Result<ModelDefaults, CatalogueError> {
    if raw.context_tokens == 0 || raw.context_tokens > MAX_CONTEXT_TOKENS {
        return Err(invalid(
            index,
            "defaults.contextTokens",
            "must be between 1 and 1048576",
        ));
    }
    if raw.output_tokens == 0 || raw.output_tokens > raw.context_tokens {
        return Err(invalid(
            index,
            "defaults.outputTokens",
            "must be between 1 and the default context size",
        ));
    }
    Ok(ModelDefaults {
        context_tokens: raw.context_tokens,
        output_tokens: raw.output_tokens,
    })
}

fn validate_capabilities(
    raw: RawModelCapabilities,
    defaults: ModelDefaults,
    index: usize,
) -> Result<ModelCapabilities, CatalogueError> {
    let unique = raw.features.iter().copied().collect::<HashSet<_>>();
    if unique.len() != raw.features.len() {
        return Err(invalid(
            index,
            "capabilities.features",
            "must not contain duplicates",
        ));
    }
    if !unique.contains(&ModelCapability::TextInput) {
        return Err(invalid(
            index,
            "capabilities.features",
            "must include textInput",
        ));
    }
    if raw.context_limit_tokens == 0
        || raw.context_limit_tokens > MAX_CONTEXT_TOKENS
        || defaults.context_tokens > raw.context_limit_tokens
    {
        return Err(invalid(
            index,
            "capabilities.contextLimitTokens",
            "must cover the default context and be at most 1048576",
        ));
    }
    Ok(ModelCapabilities {
        features: raw.features,
        context_limit_tokens: raw.context_limit_tokens,
    })
}

fn validate_ollama_distribution(
    backend: InferenceBackend,
    raw: Option<RawOllamaDistribution>,
    index: usize,
) -> Result<Option<OllamaDistribution>, CatalogueError> {
    match (backend, raw) {
        (InferenceBackend::LlamaCpp, None) => Ok(None),
        (InferenceBackend::LlamaCpp, Some(_)) => Err(invalid(
            index,
            "backend.ollama",
            "must be absent for the llama.cpp backend",
        )),
        (InferenceBackend::Ollama, None) => Err(invalid(
            index,
            "backend.ollama",
            "is required for the Ollama backend",
        )),
        (InferenceBackend::Ollama, Some(raw)) => {
            validate_ollama_tag(&raw.tag)
                .map_err(|reason| invalid(index, "backend.ollama.tag", reason))?;
            for (field, digest) in [
                ("backend.ollama.manifestSha256", &raw.manifest_sha256),
                ("backend.ollama.configSha256", &raw.config_sha256),
                ("backend.ollama.parametersSha256", &raw.parameters_sha256),
                ("backend.ollama.licenseSha256", &raw.license_sha256),
            ] {
                validate_sha256(digest).map_err(|reason| invalid(index, field, reason))?;
            }
            if raw.config_bytes == 0 || raw.parameters_bytes == 0 || raw.license_bytes == 0 {
                return Err(invalid(
                    index,
                    "backend.ollama",
                    "module sizes must be greater than zero",
                ));
            }
            validate_version_component(&raw.renderer, MAX_VERSION_BYTES)
                .map_err(|reason| invalid(index, "backend.ollama.renderer", reason))?;
            validate_version_component(&raw.parser, MAX_VERSION_BYTES)
                .map_err(|reason| invalid(index, "backend.ollama.parser", reason))?;
            Ok(Some(OllamaDistribution {
                tensor_bundle: raw
                    .tensor_bundle_manifest
                    .as_deref()
                    .map(|text| {
                        super::tensor_bundle::TensorBundle::parse(
                            text,
                            &raw.manifest_sha256,
                            &raw.config_sha256,
                            raw.config_bytes,
                            index,
                        )
                    })
                    .transpose()?,
                tag: raw.tag,
                manifest_sha256: raw.manifest_sha256,
                config_sha256: raw.config_sha256,
                config_bytes: raw.config_bytes,
                parameters_sha256: raw.parameters_sha256,
                parameters_bytes: raw.parameters_bytes,
                license_sha256: raw.license_sha256,
                license_bytes: raw.license_bytes,
                renderer: raw.renderer,
                parser: raw.parser,
            }))
        }
    }
}

fn validate_multimodal_profile(
    raw: Option<RawMultimodalProfile>,
    companions: &[CompanionArtifact],
    defaults: ModelDefaults,
    embedded_vision: bool,
    index: usize,
) -> Result<Option<MultimodalProfile>, CatalogueError> {
    raw.map(|profile| {
        let valid_projector = match profile.projector_artifact_id.as_deref() {
            None => embedded_vision,
            Some(id) => {
                validate_identifier(id, MAX_ID_BYTES).map_err(|reason| {
                    invalid(index, "multimodalProfile.projectorArtifactId", reason)
                })?;
                !embedded_vision
                    && companions.iter().any(|companion| {
                        companion.kind == CompanionKind::VisionProjector
                            && companion.artifact.key.id == id
                    })
            }
        };
        if !valid_projector {
            return Err(invalid(
                index,
                "multimodalProfile.projectorArtifactId",
                "must identify this model's vision projector",
            ));
        }
        if profile.image_tokens == 0 {
            return Err(invalid(
                index,
                "multimodalProfile.imageTokens",
                "must be greater than zero",
            ));
        }
        if profile.max_images == 0 || profile.max_images > MAX_PROFILE_IMAGES {
            return Err(invalid(
                index,
                "multimodalProfile.maxImages",
                "must be between 1 and 4",
            ));
        }
        let reserved = profile
            .image_tokens
            .checked_mul(profile.max_images)
            .and_then(|tokens| tokens.checked_add(defaults.output_tokens))
            .ok_or_else(|| image_context_error(index))?;
        if reserved > defaults.context_tokens {
            return Err(image_context_error(index));
        }
        Ok(MultimodalProfile {
            projector_artifact_id: profile.projector_artifact_id,
            image_tokens: profile.image_tokens,
            max_images: profile.max_images,
        })
    })
    .transpose()
}

const fn image_context_error(index: usize) -> CatalogueError {
    invalid(
        index,
        "multimodalProfile.imageTokens",
        "image and output tokens exceed the default context",
    )
}

fn validate_companion(
    raw: RawCompanionArtifact,
    model: &ModelKey,
    compatibility_version: &str,
    index: usize,
    field: &'static str,
) -> Result<CompanionArtifact, CatalogueError> {
    validate_version_component(&raw.compatibility_version, MAX_VERSION_BYTES)
        .map_err(|reason| invalid(index, field, reason))?;
    if raw.compatibility_version != compatibility_version {
        return Err(invalid(
            index,
            field,
            "compatibility version must match the model companion group",
        ));
    }
    if raw.memory_requirement_bytes == 0 {
        return Err(invalid(
            index,
            field,
            "memory requirement must be greater than zero",
        ));
    }
    let artifact = validate_artifact(raw.artifact, model, index)?;
    if artifact.format() != ArtifactFormat::Gguf {
        return Err(invalid(index, field, "chat companions require GGUF"));
    }
    Ok(CompanionArtifact {
        artifact,
        kind: raw.kind,
        required: raw.required,
        compatibility_version: raw.compatibility_version,
        memory_requirement_bytes: raw.memory_requirement_bytes,
    })
}

pub(super) fn validate_artifact(
    raw: RawModelArtifact,
    model: &ModelKey,
    index: usize,
) -> Result<ModelArtifact, CatalogueError> {
    validate_slug(&raw.id, MAX_ID_BYTES).map_err(|reason| invalid(index, "artifact.id", reason))?;
    validate_filename(&raw.file_name)
        .map_err(|reason| invalid(index, "artifact.fileName", reason))?;
    if raw.download_bytes == 0 {
        return Err(invalid(
            index,
            "artifact.downloadBytes",
            "must be greater than zero",
        ));
    }
    if raw.installed_bytes == 0 {
        return Err(invalid(
            index,
            "artifact.installedBytes",
            "must be greater than zero",
        ));
    }
    validate_sha256(&raw.sha256).map_err(|reason| invalid(index, "artifact.sha256", reason))?;
    let download_url = validate_https_url(&raw.download_url)
        .map_err(|reason| invalid(index, "artifact.downloadUrl", reason))?;
    Ok(ModelArtifact {
        key: ArtifactKey {
            model: model.clone(),
            id: raw.id,
        },
        file_name: raw.file_name,
        format: raw.format,
        download_url,
        download_bytes: raw.download_bytes,
        installed_bytes: raw.installed_bytes,
        sha256: raw.sha256,
    })
}

pub(super) fn validate_identifier(value: &str, max_bytes: usize) -> Result<(), &'static str> {
    validate_bounded_ascii(value, max_bytes)?;
    if value.starts_with('/')
        || value.ends_with('/')
        || value.contains("//")
        || value.split('/').any(|part| part == "." || part == "..")
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"-._/".contains(&byte)
        })
    {
        return Err("must be a canonical lowercase publisher-qualified identifier");
    }
    Ok(())
}

fn validate_slug(value: &str, max_bytes: usize) -> Result<(), &'static str> {
    validate_bounded_ascii(value, max_bytes)?;
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"-._".contains(&byte))
    {
        return Err("must contain only lowercase ASCII letters, digits, dash, dot or underscore");
    }
    Ok(())
}

pub(super) fn validate_version_component(
    value: &str,
    max_bytes: usize,
) -> Result<(), &'static str> {
    validate_bounded_ascii(value, max_bytes)?;
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"-._+".contains(&byte))
    {
        return Err("contains unsupported characters");
    }
    Ok(())
}

fn validate_ollama_tag(value: &str) -> Result<(), &'static str> {
    validate_bounded_ascii(value, MAX_VERSION_BYTES)?;
    let Some((name, tag)) = value.split_once(':') else {
        return Err("must contain an explicit model name and tag");
    };
    if name.is_empty()
        || tag.is_empty()
        || value.matches(':').count() != 1
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')
        })
    {
        return Err("must be a canonical lowercase Ollama model tag");
    }
    Ok(())
}

fn validate_filename(value: &str) -> Result<(), &'static str> {
    validate_bounded_ascii(value, MAX_FILENAME_BYTES)?;
    if value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\\')
        || value.starts_with('.')
    {
        return Err("must be a visible filename without path components");
    }
    Ok(())
}

pub(super) fn validate_text(value: &str, max_bytes: usize) -> Result<(), &'static str> {
    if value.is_empty() || value.len() > max_bytes {
        return Err("must be non-empty and within its byte limit");
    }
    if value.chars().any(char::is_control) {
        return Err("must not contain control characters");
    }
    Ok(())
}

fn validate_bounded_ascii(value: &str, max_bytes: usize) -> Result<(), &'static str> {
    if value.is_empty() || value.len() > max_bytes {
        return Err("must be non-empty and within its byte limit");
    }
    if !value.is_ascii() {
        return Err("must contain only ASCII characters");
    }
    Ok(())
}

pub(super) fn validate_sha256(value: &str) -> Result<(), &'static str> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("must be exactly 64 lowercase hexadecimal characters");
    }
    Ok(())
}

pub(super) fn validate_https_url(value: &str) -> Result<Url, &'static str> {
    let url = Url::parse(value).map_err(|_| "must be an absolute URL")?;
    if url.scheme() != "https" {
        return Err("must use HTTPS");
    }
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err("must have a host and no credentials or fragment");
    }
    Ok(url)
}

pub(super) const fn invalid(
    index: usize,
    field: &'static str,
    reason: &'static str,
) -> CatalogueError {
    CatalogueError::InvalidField {
        model_index: index,
        field,
        reason,
    }
}
