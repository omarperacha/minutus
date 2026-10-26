//! Runtime-owned aliases to external weights and pinned small provider metadata.
//! Never uploads a blob, pulls a model, or opens external weights for writing.
use crate::{ExternalModelError, ModelManifest};
use sha2::{Digest, Sha256};
use std::{fmt::Write as _, fs, io::Write as _, path::Path};

/// References verified external weights in the private runtime store.
///
/// # Errors
/// Rejects incompatible platforms, indirect store paths and conflicting aliases.
pub fn link_external_blob(
    store: &Path,
    source: &Path,
    digest: &str,
) -> Result<(), ExternalModelError> {
    if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ExternalModelError::Unsupported);
    }
    let blobs = store.join("blobs");
    private_directory(&blobs)?;
    let target = blobs.join(format!("sha256-{digest}"));
    if source == target {
        return Ok(());
    }
    if let Ok(metadata) = fs::symlink_metadata(&target) {
        if metadata.file_type().is_symlink() {
            if fs::read_link(&target).ok().as_deref() == Some(source) {
                return Ok(());
            }
            // Only the alias is removed; its target is never followed.
            fs::remove_file(&target).map_err(|_| ExternalModelError::Unavailable)?;
        } else {
            // Reuse a blob already owned by the verified managed store. Like
            // ordinary downloaded models, it is not rehashed on each response.
            // Never replace/delete managed weights to create an external alias.
            let source_bytes = fs::metadata(source)
                .map_err(|_| ExternalModelError::Unavailable)?
                .len();
            if !metadata.file_type().is_file() || metadata.len() != source_bytes {
                return Err(ExternalModelError::Changed);
            }
            return Ok(());
        }
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(source, target).map_err(|_| ExternalModelError::Unavailable)
    }
    #[cfg(not(unix))]
    {
        Err(ExternalModelError::Unsupported)
    }
}

/// Removes only an exact application-created alias, never the external target.
///
/// # Errors
/// Returns an error if an owned alias cannot be unlinked.
pub fn unlink_external_blob(
    store: &Path,
    source: &Path,
    digest: &str,
) -> Result<(), ExternalModelError> {
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ExternalModelError::Unsupported);
    }
    private_directory(&store.join("blobs"))?;
    let target = store.join("blobs").join(format!("sha256-{digest}"));
    if fs::symlink_metadata(&target).is_ok_and(|m| m.file_type().is_symlink())
        && fs::read_link(&target).ok().as_deref() == Some(source)
    {
        fs::remove_file(target).map_err(|_| ExternalModelError::Unavailable)?;
    }
    Ok(())
}

/// Installs pinned Qwen provider metadata only, with no model-weight transfer.
///
/// # Errors
/// Rejects unqualified manifests, mismatching embedded resources and store failures.
pub fn register_external_qwen_metadata(
    store: &Path,
    model: &ModelManifest,
) -> Result<(), ExternalModelError> {
    let distribution = model
        .ollama_distribution()
        .ok_or(ExternalModelError::Unsupported)?;
    if model.key().id() != "qwen/qwen3.8-27b" || distribution.tag() != "qwen3.8:27b" {
        return Err(ExternalModelError::Unsupported);
    }
    let blobs = store.join("blobs");
    private_directory(&blobs)?;
    for (data, digest) in [
        (
            include_bytes!("../../resources/qwen-gguf/config.json").as_slice(),
            distribution.config_sha256(),
        ),
        (
            include_bytes!("../../resources/qwen-gguf/params.json").as_slice(),
            distribution.parameters_sha256(),
        ),
        (
            include_bytes!("../../resources/qwen-gguf/LICENSE").as_slice(),
            distribution.license_sha256(),
        ),
    ] {
        write_metadata(&blobs.join(format!("sha256-{digest}")), data, digest)?;
    }
    let manifests = store.join("manifests/registry.ollama.ai/library/qwen3.8");
    private_directory(&manifests)?;
    write_metadata(
        &manifests.join("27b"),
        include_bytes!("../../resources/qwen-gguf/manifest.json"),
        distribution.manifest_sha256(),
    )
}

/// Removes the exact private model manifest receipt, without provider garbage collection.
///
/// # Errors
/// Returns errors removing app-owned metadata; external weights are never touched.
pub fn unregister_external_model_metadata(
    store: &Path,
    model: &ModelManifest,
) -> Result<(), ExternalModelError> {
    let relative = match model.key().id() {
        "qwen/qwen3.8-27b" => "manifests/registry.ollama.ai/library/qwen3.8/27b",
        "meta-models/muse-glimmer-30b" => {
            "manifests/registry.ollama.ai/zeta/muse-glimmer-30b/4cc57c0f5104-v2"
        }
        _ => return Ok(()),
    };
    let manifest = store.join(relative);
    let directory = manifest.parent().ok_or(ExternalModelError::Unsupported)?;
    private_directory(directory)?;
    if fs::symlink_metadata(&manifest).is_ok() {
        // Unlink the private entry itself. Never follow an entry to an external file.
        fs::remove_file(manifest).map_err(|_| ExternalModelError::Unavailable)?;
    }
    Ok(())
}

pub(super) fn private_directory(path: &Path) -> Result<(), ExternalModelError> {
    // Reject indirect existing ancestors before creating anything beneath them.
    for ancestor in path.ancestors() {
        if let Ok(metadata) = fs::symlink_metadata(ancestor)
            && (!metadata.is_dir() || metadata.file_type().is_symlink())
        {
            return Err(ExternalModelError::Unavailable);
        }
    }
    fs::create_dir_all(path).map_err(|_| ExternalModelError::Unavailable)
}

pub(super) fn write_metadata(
    path: &Path,
    data: &[u8],
    digest: &str,
) -> Result<(), ExternalModelError> {
    if format!("{:x}", Sha256::digest(data)) != digest {
        return Err(ExternalModelError::Unsupported);
    }
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.file_type().is_file() {
            return Err(ExternalModelError::Unavailable);
        }
        if fs::read(path).ok().as_deref() == Some(data) {
            return Ok(());
        }
    }
    // Publish small app-owned metadata atomically, never writing through an entry
    // that could have been replaced with a symlink after the check above.
    let mut random = [0_u8; 16];
    getrandom::fill(&mut random).map_err(|_| ExternalModelError::Unavailable)?;
    let mut suffix = String::with_capacity(32);
    for byte in random {
        write!(&mut suffix, "{byte:02x}").map_err(|_| ExternalModelError::Unavailable)?;
    }
    let temporary = path.with_file_name(format!(".external-metadata-{suffix}.tmp"));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|_| ExternalModelError::Unavailable)?;
    let result = (|| {
        file.write_all(data)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(|_| ExternalModelError::Unavailable)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn existing_managed_blob_is_preserved_and_size_conflicts_are_rejected()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let store = root.path().canonicalize()?.join("store");
        private_directory(&store.join("blobs"))?;
        let source = root.path().join("selected.gguf");
        fs::write(&source, b"weights")?;
        let digest = format!("{:x}", Sha256::digest(b"weights"));
        let managed = store.join("blobs").join(format!("sha256-{digest}"));
        fs::write(&managed, b"weights")?;
        link_external_blob(&store, &source, &digest)?;
        assert!(fs::symlink_metadata(&managed)?.file_type().is_file());
        fs::write(&managed, b"wrong size")?;
        assert_eq!(
            link_external_blob(&store, &source, &digest),
            Err(ExternalModelError::Changed)
        );
        assert_eq!(fs::read(&managed)?, b"wrong size");
        assert_eq!(fs::read(&source)?, b"weights");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn alias_relocation_and_unregistration_never_copy_or_remove_weights()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let root_path = root.path().canonicalize()?;
        let source = root_path.join("original.gguf");
        let moved = root_path.join("moved.gguf");
        let store = root_path.join("store");
        fs::write(&source, b"weights")?;
        link_external_blob(&store, &source, &"a".repeat(64))?;
        let alias = store
            .join("blobs")
            .join(format!("sha256-{}", "a".repeat(64)));
        assert!(fs::symlink_metadata(&alias)?.file_type().is_symlink());
        fs::rename(&source, &moved)?;
        assert!(!alias.exists());
        link_external_blob(&store, &moved, &"a".repeat(64))?;
        assert_eq!(fs::read(&alias)?, b"weights");
        unlink_external_blob(&store, &moved, &"a".repeat(64))?;
        assert!(fs::symlink_metadata(alias).is_err());
        assert_eq!(fs::read(moved)?, b"weights");
        Ok(())
    }
    #[test]
    fn embedded_qwen_metadata_matches_catalogue() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let catalogue = crate::bundled_catalogue()?;
        let model = catalogue
            .models()
            .iter()
            .find(|m| m.key().id() == "qwen/qwen3.8-27b")
            .ok_or("missing Qwen")?;
        register_external_qwen_metadata(&root.path().canonicalize()?, model)?;
        assert!(
            !root
                .path()
                .join("blobs")
                .join(format!("sha256-{}", model.artifact().sha256()))
                .exists()
        );
        Ok(())
    }
}
