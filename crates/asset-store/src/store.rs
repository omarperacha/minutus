//! Atomic managed-file publication and orphan reconciliation.

use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

use app_storage::{AssetFileReference, AssetReference};
use ipc_types::AssetId;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{AssetStoreError, media};

const ORIGINALS_DIRECTORY: &str = "originals";
const INFERENCE_DIRECTORY: &str = "inference";
const STAGING_DIRECTORY: &str = "staging";

/// Files published for an asset but not yet committed to application metadata.
///
/// Callers persist [`Self::metadata`] and then consume this value with
/// [`Self::commit`]. Dropping an uncommitted value removes its files best effort.
pub struct ImportedAsset {
    metadata: AssetReference,
    root: PathBuf,
    committed: bool,
}

impl ImportedAsset {
    /// Returns metadata suitable for one atomic application-database write.
    #[must_use]
    pub fn metadata(&self) -> &AssetReference {
        &self.metadata
    }

    /// Marks the files as owned by committed application metadata.
    #[must_use]
    pub fn commit(mut self) -> AssetReference {
        self.committed = true;
        self.metadata.clone()
    }
}

impl Drop for ImportedAsset {
    fn drop(&mut self) {
        if !self.committed {
            remove_managed_file(&self.root, &self.metadata.original.storage_key);
            remove_managed_file(&self.root, &self.metadata.derivative.storage_key);
        }
    }
}

/// Private filesystem store for original and normalized image bytes.
pub struct AssetFileStore {
    root: PathBuf,
}

impl AssetFileStore {
    /// Creates private managed directories and clears abandoned staging files.
    ///
    /// # Errors
    ///
    /// Returns an error when the private directories cannot be prepared.
    pub fn initialize(root: impl AsRef<Path>) -> Result<Self, AssetStoreError> {
        let root = root.as_ref().to_path_buf();
        create_private_directory(&root)?;
        for directory in [ORIGINALS_DIRECTORY, INFERENCE_DIRECTORY, STAGING_DIRECTORY] {
            create_private_directory(&root.join(directory))?;
        }
        let store = Self { root };
        store.clear_staging()?;
        Ok(store)
    }

    /// Validates bytes and atomically publishes separate original and inference files.
    ///
    /// The input has no source-path or URL field. The returned guard deletes both
    /// files unless its metadata is successfully persisted and committed.
    ///
    /// # Errors
    ///
    /// Returns a sanitized error for unsupported, malformed, oversized or
    /// unpersistable input.
    pub fn import_image(
        &self,
        bytes: &[u8],
        imported_at: &str,
    ) -> Result<ImportedAsset, AssetStoreError> {
        let normalized = media::normalize(bytes)?;
        let id = AssetId(Uuid::now_v7().to_string());
        let original_key = format!(
            "{ORIGINALS_DIRECTORY}/{}.{}",
            id.0,
            normalized.source_format.extension()
        );
        let derivative_key = format!("{INFERENCE_DIRECTORY}/{}.png", id.0);
        let original_path = resolve_key(&self.root, &original_key)?;
        let derivative_path = resolve_key(&self.root, &derivative_key)?;
        let original_stage = self
            .root
            .join(STAGING_DIRECTORY)
            .join(format!("{}.original.tmp", id.0));
        let derivative_stage = self
            .root
            .join(STAGING_DIRECTORY)
            .join(format!("{}.inference.tmp", id.0));

        let publish_result = (|| {
            write_private_file(&original_stage, bytes)?;
            write_private_file(&derivative_stage, &normalized.derivative_png)?;
            fs::rename(&original_stage, &original_path)?;
            sync_directory(
                original_path
                    .parent()
                    .ok_or(AssetStoreError::InvalidStorageKey)?,
            )?;
            fs::rename(&derivative_stage, &derivative_path)?;
            sync_directory(
                derivative_path
                    .parent()
                    .ok_or(AssetStoreError::InvalidStorageKey)?,
            )?;
            Ok::<(), AssetStoreError>(())
        })();
        if let Err(error) = publish_result {
            remove_file_if_present(&original_stage);
            remove_file_if_present(&derivative_stage);
            remove_file_if_present(&original_path);
            remove_file_if_present(&derivative_path);
            return Err(error);
        }

        let original = AssetFileReference {
            storage_key: original_key,
            media_type: normalized.source_format.media_type().to_owned(),
            content_digest: digest_hex(bytes),
            byte_size: u64::try_from(bytes.len()).map_err(|_| AssetStoreError::LimitExceeded)?,
            width: normalized.width,
            height: normalized.height,
        };
        let derivative = AssetFileReference {
            storage_key: derivative_key,
            media_type: "image/png".to_owned(),
            content_digest: digest_hex(&normalized.derivative_png),
            byte_size: u64::try_from(normalized.derivative_png.len())
                .map_err(|_| AssetStoreError::LimitExceeded)?,
            width: normalized.derivative_width,
            height: normalized.derivative_height,
        };
        Ok(ImportedAsset {
            metadata: AssetReference {
                id,
                original,
                derivative,
                imported_at: imported_at.to_owned(),
            },
            root: self.root.clone(),
            committed: false,
        })
    }

    /// Reads and verifies the normalized bytes passed to local inference.
    ///
    /// # Errors
    ///
    /// Returns an error when the Rust-owned key is invalid, the file is missing,
    /// or its size or digest no longer matches committed metadata.
    pub fn read_inference(&self, asset: &AssetReference) -> Result<Vec<u8>, AssetStoreError> {
        let path = resolve_key(&self.root, &asset.derivative.storage_key)?;
        let bytes = read_private_file(&path)?;
        if u64::try_from(bytes.len()).map_err(|_| AssetStoreError::LimitExceeded)?
            != asset.derivative.byte_size
            || digest_hex(&bytes) != asset.derivative.content_digest
        {
            return Err(AssetStoreError::InvalidMedia);
        }
        Ok(bytes)
    }

    /// Removes both files for database metadata already reaped as unreferenced.
    ///
    /// Missing files are treated as an already-completed cleanup step.
    ///
    /// # Errors
    ///
    /// Returns an error when a managed key is invalid or a remaining file cannot
    /// be removed. A later reconciliation can safely retry partial cleanup.
    pub fn remove_assets(&self, assets: &[AssetReference]) -> Result<(), AssetStoreError> {
        let mut first_error = None;
        for asset in assets {
            for key in [&asset.original.storage_key, &asset.derivative.storage_key] {
                match remove_key(&self.root, key) {
                    Err(error) if first_error.is_none() => first_error = Some(error),
                    Ok(()) | Err(_) => {}
                }
            }
        }
        sync_directory(&self.root.join(ORIGINALS_DIRECTORY))?;
        sync_directory(&self.root.join(INFERENCE_DIRECTORY))?;
        first_error.map_or(Ok(()), Err)
    }

    /// Removes files that have no committed database storage key.
    ///
    /// # Errors
    ///
    /// Returns an error when managed directories cannot be read or cleaned.
    pub fn reconcile(&self, assets: &[AssetReference]) -> Result<(), AssetStoreError> {
        self.clear_staging()?;
        let live = assets
            .iter()
            .flat_map(|asset| {
                [
                    asset.original.storage_key.as_str(),
                    asset.derivative.storage_key.as_str(),
                ]
            })
            .collect::<HashSet<_>>();
        for directory in [ORIGINALS_DIRECTORY, INFERENCE_DIRECTORY] {
            let path = self.root.join(directory);
            for entry in fs::read_dir(&path)? {
                let entry = entry?;
                let file_name = entry.file_name();
                let name = file_name
                    .to_str()
                    .ok_or(AssetStoreError::InvalidStorageKey)?;
                let key = format!("{directory}/{name}");
                if !live.contains(key.as_str()) {
                    remove_file_strict(&entry.path())?;
                }
            }
            sync_directory(&path)?;
        }
        Ok(())
    }

    fn clear_staging(&self) -> Result<(), AssetStoreError> {
        let staging = self.root.join(STAGING_DIRECTORY);
        for entry in fs::read_dir(&staging)? {
            let entry = entry?;
            let metadata = fs::symlink_metadata(entry.path())?;
            if metadata.is_file() || metadata.file_type().is_symlink() {
                remove_file_strict(&entry.path())?;
            } else {
                return Err(AssetStoreError::InvalidStorageKey);
            }
        }
        sync_directory(&staging)?;
        Ok(())
    }
}

fn digest_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn resolve_key(root: &Path, key: &str) -> Result<PathBuf, AssetStoreError> {
    let mut components = Path::new(key).components();
    let Some(Component::Normal(directory)) = components.next() else {
        return Err(AssetStoreError::InvalidStorageKey);
    };
    if directory != ORIGINALS_DIRECTORY && directory != INFERENCE_DIRECTORY {
        return Err(AssetStoreError::InvalidStorageKey);
    }
    let Some(Component::Normal(file_name)) = components.next() else {
        return Err(AssetStoreError::InvalidStorageKey);
    };
    if components.next().is_some() {
        return Err(AssetStoreError::InvalidStorageKey);
    }
    let file_name = file_name
        .to_str()
        .ok_or(AssetStoreError::InvalidStorageKey)?;
    let stem = file_name
        .split_once('.')
        .map(|(stem, _)| stem)
        .ok_or(AssetStoreError::InvalidStorageKey)?;
    Uuid::parse_str(stem).map_err(|_| AssetStoreError::InvalidStorageKey)?;
    Ok(root.join(directory).join(file_name))
}

fn create_private_directory(path: &Path) -> Result<(), AssetStoreError> {
    fs::create_dir_all(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(AssetStoreError::InvalidStorageKey);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn read_private_file(path: &Path) -> Result<Vec<u8>, AssetStoreError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(AssetStoreError::InvalidStorageKey);
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), AssetStoreError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn remove_key(root: &Path, key: &str) -> Result<(), AssetStoreError> {
    let path = resolve_key(root, key)?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn remove_file_strict(path: &Path) -> Result<(), AssetStoreError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn remove_managed_file(root: &Path, key: &str) {
    if let Ok(path) = resolve_key(root, key) {
        remove_file_if_present(&path);
    }
}

fn remove_file_if_present(path: &Path) {
    // Best-effort rollback is followed by startup reconciliation.
    let _result = fs::remove_file(path);
}

fn sync_directory(path: &Path) -> Result<(), AssetStoreError> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
    use tempfile::TempDir;

    use super::*;

    fn source_png() -> Result<Vec<u8>, image::ImageError> {
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(RgbImage::from_pixel(8, 6, Rgb([1, 2, 3])))
            .write_to(&mut bytes, ImageFormat::Png)?;
        Ok(bytes.into_inner())
    }

    #[test]
    fn stores_original_and_derivative_under_opaque_private_keys()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TempDir::new()?;
        let store = AssetFileStore::initialize(directory.path())?;
        let import = store.import_image(&source_png()?, "2026-08-14T12:00:00Z")?;
        let metadata = import.metadata().clone();
        assert_ne!(
            metadata.original.storage_key,
            metadata.derivative.storage_key
        );
        assert_eq!(metadata.original.media_type, "image/png");
        assert_eq!(metadata.derivative.media_type, "image/png");
        assert_eq!(
            store.read_inference(&metadata)?.len() as u64,
            metadata.derivative.byte_size
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let original = directory.path().join(&metadata.original.storage_key);
            assert_eq!(fs::metadata(original)?.permissions().mode() & 0o777, 0o600);
            assert_eq!(
                fs::metadata(directory.path().join(INFERENCE_DIRECTORY))?
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        let metadata = import.commit();
        store.remove_assets(&[metadata])?;
        Ok(())
    }

    #[test]
    fn dropped_import_rolls_back_and_reconciliation_removes_orphans()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TempDir::new()?;
        let store = AssetFileStore::initialize(directory.path())?;
        let metadata = {
            let import = store.import_image(&source_png()?, "2026-08-14T12:00:00Z")?;
            import.metadata().clone()
        };
        assert!(store.read_inference(&metadata).is_err());

        let committed = store
            .import_image(&source_png()?, "2026-08-14T12:00:00Z")?
            .commit();
        assert!(store.read_inference(&committed).is_ok());
        store.reconcile(&[])?;
        assert!(store.read_inference(&committed).is_err());
        Ok(())
    }

    #[test]
    fn rejects_non_generated_storage_keys() -> Result<(), Box<dyn std::error::Error>> {
        let directory = TempDir::new()?;
        assert!(resolve_key(directory.path(), "../outside").is_err());
        assert!(resolve_key(directory.path(), "/tmp/image.png").is_err());
        assert!(resolve_key(directory.path(), "inference/not-a-uuid.png").is_err());
        Ok(())
    }

    #[test]
    fn inference_read_rejects_modified_managed_bytes() -> Result<(), Box<dyn std::error::Error>> {
        let directory = TempDir::new()?;
        let store = AssetFileStore::initialize(directory.path())?;
        let asset = store
            .import_image(&source_png()?, "2026-08-14T12:00:00Z")?
            .commit();
        let derivative = directory.path().join(&asset.derivative.storage_key);
        fs::write(derivative, b"modified")?;
        assert!(matches!(
            store.read_inference(&asset),
            Err(AssetStoreError::InvalidMedia)
        ));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn inference_read_never_follows_a_replaced_symlink() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        let directory = TempDir::new()?;
        let store = AssetFileStore::initialize(directory.path())?;
        let asset = store
            .import_image(&source_png()?, "2026-08-14T12:00:00Z")?
            .commit();
        let derivative = directory.path().join(&asset.derivative.storage_key);
        let outside = directory.path().join("outside.png");
        fs::write(&outside, source_png()?)?;
        fs::remove_file(&derivative)?;
        symlink(&outside, &derivative)?;
        assert!(store.read_inference(&asset).is_err());
        Ok(())
    }
}
