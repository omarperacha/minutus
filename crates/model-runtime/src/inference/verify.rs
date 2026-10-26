//! Full integrity and compatibility verification immediately before model load.

use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
};

use sha2::{Digest, Sha256};

use super::{InferenceError, ModelLoadConfig, PINNED_LLAMA_CPP_VERSION};
use crate::gguf::inspect_gguf_file;

const HASH_BUFFER_BYTES: usize = 1024 * 1024;
const SHA256_HEX_BYTES: usize = 64;

pub(super) struct VerifiedLaunch {
    pub(super) executable: PathBuf,
    pub(super) model: PathBuf,
    pub(super) context_tokens: u32,
    pub(super) output_tokens: u32,
    pub(super) companions: Vec<VerifiedCompanion>,
}

pub(super) struct VerifiedCompanion {
    pub(super) kind: crate::CompanionKind,
    pub(super) path: PathBuf,
}

pub(super) fn verify(config: &ModelLoadConfig) -> Result<VerifiedLaunch, InferenceError> {
    validate_config(config)?;
    reject_symlink(&config.model)?;
    reject_symlink(&config.executable)?;
    let executable = config
        .executable
        .canonicalize()
        .map_err(InferenceError::ArtifactIo)?;
    if !executable.is_file() {
        return Err(InferenceError::InvalidExecutable);
    }
    let model = config
        .model
        .canonicalize()
        .map_err(InferenceError::ArtifactIo)?;
    let mut file = File::open(&model).map_err(InferenceError::ArtifactIo)?;
    let before = file.metadata().map_err(InferenceError::ArtifactIo)?;
    if !before.file_type().is_file() || before.len() != config.expected_bytes {
        return Err(InferenceError::ArtifactSizeMismatch);
    }
    let metadata = inspect_gguf_file(&mut file).map_err(InferenceError::Gguf)?;
    if config.context_tokens > metadata.context_tokens() {
        return Err(InferenceError::ContextLimitExceeded);
    }
    file.seek(SeekFrom::Start(0))
        .map_err(InferenceError::ArtifactIo)?;
    let actual_digest = hash_reader(&mut file)?;
    let after = file.metadata().map_err(InferenceError::ArtifactIo)?;
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err(InferenceError::ArtifactChanged);
    }
    if actual_digest != config.expected_sha256 {
        return Err(InferenceError::ArtifactDigestMismatch);
    }
    let companions = config
        .companions
        .iter()
        .map(|companion| {
            let path = verify_artifact(
                &companion.artifact,
                &companion.expected_sha256,
                companion.expected_bytes,
            )?;
            Ok(VerifiedCompanion {
                kind: companion.kind,
                path,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(VerifiedLaunch {
        executable,
        model,
        context_tokens: config.context_tokens,
        output_tokens: config.output_tokens,
        companions,
    })
}

fn validate_config(config: &ModelLoadConfig) -> Result<(), InferenceError> {
    if config.expected_sha256.len() != SHA256_HEX_BYTES
        || !config
            .expected_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(InferenceError::InvalidDigest);
    }
    if config.expected_bytes == 0 {
        return Err(InferenceError::ArtifactSizeMismatch);
    }
    if config.context_tokens == 0
        || config.output_tokens == 0
        || config.output_tokens > config.context_tokens
    {
        return Err(InferenceError::InvalidLimits);
    }
    if config
        .backend_compatibility_version
        .as_deref()
        .is_some_and(|version| version != PINNED_LLAMA_CPP_VERSION)
    {
        return Err(InferenceError::IncompatibleBackend);
    }
    let mut kinds = std::collections::HashSet::new();
    for companion in &config.companions {
        if !kinds.insert(companion.kind) {
            return Err(InferenceError::DuplicateCompanionKind);
        }
        if config.companion_compatibility_version.as_deref()
            != Some(companion.compatibility_version.as_str())
        {
            return Err(InferenceError::IncompatibleCompanion);
        }
    }
    if config.companions.is_empty() && config.companion_compatibility_version.is_some() {
        return Err(InferenceError::IncompatibleCompanion);
    }
    Ok(())
}

fn verify_artifact(
    path: &std::path::Path,
    expected_sha256: &str,
    expected_bytes: u64,
) -> Result<PathBuf, InferenceError> {
    if expected_sha256.len() != SHA256_HEX_BYTES
        || !expected_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(InferenceError::InvalidDigest);
    }
    if expected_bytes == 0 {
        return Err(InferenceError::ArtifactSizeMismatch);
    }
    reject_symlink(path)?;
    let canonical = path.canonicalize().map_err(InferenceError::ArtifactIo)?;
    let mut file = File::open(&canonical).map_err(InferenceError::ArtifactIo)?;
    let before = file.metadata().map_err(InferenceError::ArtifactIo)?;
    if !before.file_type().is_file() || before.len() != expected_bytes {
        return Err(InferenceError::ArtifactSizeMismatch);
    }
    let actual_digest = hash_reader(&mut file)?;
    let after = file.metadata().map_err(InferenceError::ArtifactIo)?;
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err(InferenceError::ArtifactChanged);
    }
    if actual_digest != expected_sha256 {
        return Err(InferenceError::ArtifactDigestMismatch);
    }
    Ok(canonical)
}

fn reject_symlink(path: &std::path::Path) -> Result<(), InferenceError> {
    let metadata = fs::symlink_metadata(path).map_err(InferenceError::ArtifactIo)?;
    if metadata.file_type().is_symlink() {
        return Err(InferenceError::ArtifactNotRegularFile);
    }
    Ok(())
}

fn hash_reader(reader: &mut impl Read) -> Result<String, InferenceError> {
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; HASH_BUFFER_BYTES];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(InferenceError::ArtifactIo)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}
