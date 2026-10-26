//! Managed import of user-selected, supported GGUF model files.

use std::{
    error::Error,
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};

use sha2::{Digest, Sha256};
use std::fmt::Write as _;

use crate::gguf::{GgufError, GgufMetadata, inspect_gguf_file};

const COPY_BUFFER_BYTES: usize = 1024 * 1024;
const PROGRESS_INTERVAL_BYTES: u64 = 16 * 1024 * 1024;

/// Snapshot emitted while a selected GGUF is copied into managed storage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImportProgress {
    copied_bytes: u64,
    total_bytes: u64,
}

impl ImportProgress {
    /// Returns bytes copied into the managed candidate.
    #[must_use]
    pub const fn copied_bytes(self) -> u64 {
        self.copied_bytes
    }

    /// Returns the stable source length observed before copying.
    #[must_use]
    pub const fn total_bytes(self) -> u64 {
        self.total_bytes
    }

    /// Returns bytes still to copy before reinspection and hashing complete.
    #[must_use]
    pub const fn remaining_bytes(self) -> u64 {
        self.total_bytes.saturating_sub(self.copied_bytes)
    }
}

/// One compatible GGUF copied and published under Rust-managed storage.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportedModelArtifact {
    path: PathBuf,
    byte_size: u64,
    sha256: String,
    metadata: GgufMetadata,
}

impl ImportedModelArtifact {
    /// Returns the managed installed path, never the user-selected source path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the exact copied file length.
    #[must_use]
    pub const fn byte_size(&self) -> u64 {
        self.byte_size
    }

    /// Returns the lowercase SHA-256 of the managed bytes.
    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// Returns metadata reinspected from the managed copy.
    #[must_use]
    pub const fn metadata(&self) -> &GgufMetadata {
        &self.metadata
    }
}

/// Rust-owned importer for compatible user-selected GGUF files.
pub struct ModelImportManager {
    importing_directory: PathBuf,
    installed_directory: PathBuf,
    operation: Mutex<()>,
}

impl ModelImportManager {
    /// Prepares private importing and installed directories.
    ///
    /// # Errors
    ///
    /// Returns an error if managed storage cannot be created or stale import
    /// candidates cannot be removed safely.
    pub fn initialize(root: impl AsRef<Path>) -> Result<Self, ImportError> {
        let root = root.as_ref();
        create_private_directory(root)?;
        let root = root.canonicalize().map_err(ImportError::Storage)?;
        let importing_directory = root.join("importing");
        let installed_directory = root.join("installed");
        create_private_directory(&importing_directory)?;
        create_private_directory(&installed_directory)?;
        cleanup_stale_imports(&importing_directory)?;
        Ok(Self {
            importing_directory,
            installed_directory,
            operation: Mutex::new(()),
        })
    }

    /// Inspects, copies, hashes, reinspects and atomically publishes a GGUF.
    ///
    /// The source is opened read-only and is never renamed, truncated or
    /// otherwise modified. No source path is retained in the result.
    ///
    /// # Errors
    ///
    /// Returns a typed error for unsupported/malformed GGUF data, insufficient
    /// disk space, a concurrently changed source or managed-storage failure.
    pub fn import(
        &self,
        source_path: impl AsRef<Path>,
        mut progress: impl FnMut(ImportProgress),
    ) -> Result<ImportedModelArtifact, ImportError> {
        let _operation = self
            .operation
            .lock()
            .map_err(|_| ImportError::Unavailable)?;
        let mut source = File::open(source_path).map_err(ImportError::Source)?;
        let source_metadata = source.metadata().map_err(ImportError::Source)?;
        if !source_metadata.file_type().is_file() {
            return Err(ImportError::SourceNotRegularFile);
        }
        let source_size = source_metadata.len();
        let inspected = inspect_gguf_file(&mut source)?;
        let available =
            fs2::available_space(&self.importing_directory).map_err(ImportError::Storage)?;
        if available < source_size {
            return Err(ImportError::InsufficientDiskSpace {
                required: source_size,
                available,
            });
        }

        let temporary_path = self.temporary_path()?;
        let mut pending = PendingImport::create(temporary_path)?;
        source
            .seek(SeekFrom::Start(0))
            .map_err(ImportError::Source)?;
        progress(ImportProgress {
            copied_bytes: 0,
            total_bytes: source_size,
        });
        let sha256 = copy_and_hash(&mut source, pending.file_mut()?, source_size, &mut progress)?;
        pending
            .file_mut()?
            .sync_all()
            .map_err(ImportError::Storage)?;

        if source.metadata().map_err(ImportError::Source)?.len() != source_size {
            return Err(ImportError::SourceChanged);
        }
        let managed_metadata = inspect_gguf_file(pending.file_mut()?)?;
        if managed_metadata != inspected {
            return Err(ImportError::SourceChanged);
        }
        let installed_path = self.installed_directory.join(format!("{sha256}.gguf"));
        if installed_path.exists() {
            verify_existing(&installed_path, &sha256, &managed_metadata)?;
            pending.discard()?;
        } else {
            pending.publish(&installed_path)?;
            sync_directory(&self.installed_directory)?;
        }
        Ok(ImportedModelArtifact {
            path: installed_path,
            byte_size: source_size,
            sha256,
            metadata: managed_metadata,
        })
    }

    /// Reports whether metadata names this manager's digest-addressed artifact.
    ///
    /// This is a cheap ownership check only; inference still performs complete
    /// hashing and GGUF inspection immediately before load.
    #[must_use]
    pub fn is_published(&self, path: &Path, sha256: &str, byte_size: u64) -> bool {
        if sha256.len() != 64
            || !sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || path != self.installed_directory.join(format!("{sha256}.gguf"))
        {
            return false;
        }
        fs::symlink_metadata(path)
            .is_ok_and(|metadata| metadata.file_type().is_file() && metadata.len() == byte_size)
    }

    fn temporary_path(&self) -> Result<PathBuf, ImportError> {
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random).map_err(|_| ImportError::RandomnessUnavailable)?;
        let mut name = String::with_capacity(32);
        for byte in random {
            write!(&mut name, "{byte:02x}").map_err(|_| ImportError::Unavailable)?;
        }
        Ok(self.importing_directory.join(format!("import-{name}.part")))
    }
}

fn copy_and_hash(
    source: &mut File,
    destination: &mut File,
    expected_bytes: u64,
    progress: &mut impl FnMut(ImportProgress),
) -> Result<String, ImportError> {
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    let mut copied = 0_u64;
    let mut last_reported = 0_u64;
    loop {
        let read = source.read(&mut buffer).map_err(ImportError::Source)?;
        if read == 0 {
            break;
        }
        let read_u64 = u64::try_from(read).map_err(|_| ImportError::SourceChanged)?;
        copied = copied
            .checked_add(read_u64)
            .ok_or(ImportError::SourceChanged)?;
        if copied > expected_bytes {
            return Err(ImportError::SourceChanged);
        }
        destination
            .write_all(&buffer[..read])
            .map_err(ImportError::Storage)?;
        digest.update(&buffer[..read]);
        if copied == expected_bytes
            || copied.saturating_sub(last_reported) >= PROGRESS_INTERVAL_BYTES
        {
            progress(ImportProgress {
                copied_bytes: copied,
                total_bytes: expected_bytes,
            });
            last_reported = copied;
        }
    }
    if copied != expected_bytes {
        return Err(ImportError::SourceChanged);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn verify_existing(
    path: &Path,
    expected_sha256: &str,
    expected_metadata: &GgufMetadata,
) -> Result<(), ImportError> {
    let mut file = File::open(path).map_err(ImportError::Storage)?;
    let metadata =
        inspect_gguf_file(&mut file).map_err(|_| ImportError::ExistingArtifactInvalid)?;
    if &metadata != expected_metadata {
        return Err(ImportError::ExistingArtifactInvalid);
    }
    file.seek(SeekFrom::Start(0))
        .map_err(ImportError::Storage)?;
    let actual = hash_reader(&mut file).map_err(ImportError::Storage)?;
    if actual != expected_sha256 {
        return Err(ImportError::ExistingArtifactInvalid);
    }
    Ok(())
}

fn hash_reader(reader: &mut impl Read) -> io::Result<String> {
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

struct PendingImport {
    path: PathBuf,
    file: Option<File>,
}

impl PendingImport {
    fn create(path: PathBuf) -> Result<Self, ImportError> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path).map_err(ImportError::Storage)?;
        Ok(Self {
            path,
            file: Some(file),
        })
    }

    fn file_mut(&mut self) -> Result<&mut File, ImportError> {
        self.file.as_mut().ok_or(ImportError::Unavailable)
    }

    fn discard(mut self) -> Result<(), ImportError> {
        drop(self.file.take());
        fs::remove_file(&self.path).map_err(ImportError::Storage)
    }

    fn publish(mut self, destination: &Path) -> Result<(), ImportError> {
        drop(self.file.take());
        fs::rename(&self.path, destination).map_err(ImportError::Storage)
    }
}

impl Drop for PendingImport {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}

fn cleanup_stale_imports(directory: &Path) -> Result<(), ImportError> {
    for entry in fs::read_dir(directory).map_err(ImportError::Storage)? {
        let entry = entry.map_err(ImportError::Storage)?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let metadata = fs::symlink_metadata(entry.path()).map_err(ImportError::Storage)?;
        if name.starts_with("import-") && name.ends_with(".part") && metadata.file_type().is_file()
        {
            fs::remove_file(entry.path()).map_err(ImportError::Storage)?;
        }
    }
    Ok(())
}

fn create_private_directory(path: &Path) -> Result<(), ImportError> {
    fs::create_dir_all(path).map_err(ImportError::Storage)?;
    #[cfg(unix)]
    fs::set_permissions(path, unix_permissions(0o700)).map_err(ImportError::Storage)?;
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), ImportError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(ImportError::Storage)
}

#[cfg(unix)]
fn unix_permissions(mode: u32) -> fs::Permissions {
    use std::os::unix::fs::PermissionsExt;
    fs::Permissions::from_mode(mode)
}

/// Failure while importing a user-selected GGUF model.
#[derive(Debug)]
pub enum ImportError {
    /// The selected source could not be read.
    Source(io::Error),
    /// Managed import storage could not be prepared or updated.
    Storage(io::Error),
    /// GGUF inspection rejected malformed or incompatible model data.
    Gguf(GgufError),
    /// The selected source is not a regular file.
    SourceNotRegularFile,
    /// The source changed while it was being copied.
    SourceChanged,
    /// Available disk space cannot hold the managed copy.
    InsufficientDiskSpace {
        /// Bytes required for the managed copy.
        required: u64,
        /// Bytes available to the application.
        available: u64,
    },
    /// Secure temporary-name generation failed.
    RandomnessUnavailable,
    /// Another thread failed while holding the import lock.
    Unavailable,
    /// A digest-addressed installed file exists but no longer matches its digest.
    ExistingArtifactInvalid,
}

impl fmt::Display for ImportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source(_) => formatter.write_str("the selected GGUF could not be read"),
            Self::Storage(_) => formatter.write_str("managed model storage is unavailable"),
            Self::Gguf(error) => error.fmt(formatter),
            Self::SourceNotRegularFile => {
                formatter.write_str("the selected GGUF is not a regular file")
            }
            Self::SourceChanged => {
                formatter.write_str("the selected GGUF changed while it was being imported")
            }
            Self::InsufficientDiskSpace {
                required,
                available,
            } => write!(
                formatter,
                "insufficient disk space for model import: {required} bytes required, {available} bytes available"
            ),
            Self::RandomnessUnavailable => {
                formatter.write_str("a private import filename could not be generated")
            }
            Self::Unavailable => formatter.write_str("model imports are unavailable"),
            Self::ExistingArtifactInvalid => {
                formatter.write_str("an existing managed model failed integrity verification")
            }
        }
    }
}

impl Error for ImportError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Source(error) | Self::Storage(error) => Some(error),
            Self::Gguf(error) => Some(error),
            _ => None,
        }
    }
}

impl From<GgufError> for ImportError {
    fn from(error: GgufError) -> Self {
        Self::Gguf(error)
    }
}

#[cfg(test)]
mod tests {
    use std::{error::Error, fs};

    use tempfile::TempDir;

    use super::*;

    const TYPE_UINT32: u32 = 4;
    const TYPE_STRING: u32 = 8;

    fn append_string(bytes: &mut Vec<u8>, value: &str) -> Result<(), io::Error> {
        let length = u64::try_from(value.len()).map_err(io::Error::other)?;
        bytes.extend_from_slice(&length.to_le_bytes());
        bytes.extend_from_slice(value.as_bytes());
        Ok(())
    }

    fn append_string_metadata(
        bytes: &mut Vec<u8>,
        key: &str,
        value: &str,
    ) -> Result<(), io::Error> {
        append_string(bytes, key)?;
        bytes.extend_from_slice(&TYPE_STRING.to_le_bytes());
        append_string(bytes, value)
    }

    fn append_u32_metadata(bytes: &mut Vec<u8>, key: &str, value: u32) -> Result<(), io::Error> {
        append_string(bytes, key)?;
        bytes.extend_from_slice(&TYPE_UINT32.to_le_bytes());
        bytes.extend_from_slice(&value.to_le_bytes());
        Ok(())
    }

    fn valid_gguf() -> Result<Vec<u8>, io::Error> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"GGUF");
        bytes.extend_from_slice(&3_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u64.to_le_bytes());
        bytes.extend_from_slice(&5_u64.to_le_bytes());
        append_string_metadata(&mut bytes, "general.architecture", "muse-glimmer")?;
        append_string_metadata(&mut bytes, "general.type", "model")?;
        append_string_metadata(&mut bytes, "general.name", "Imported Glimmer")?;
        append_u32_metadata(&mut bytes, "muse-glimmer.context_length", 131_072)?;
        append_u32_metadata(&mut bytes, "general.alignment", 32)?;
        append_string(&mut bytes, "token_embd.weight")?;
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u64.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&0_u64.to_le_bytes());
        while bytes.len() % 32 != 0 {
            bytes.push(0);
        }
        bytes.extend_from_slice(&[1, 2, 3, 4]);
        Ok(bytes)
    }

    #[test]
    fn import_preserves_source_and_publishes_hashed_managed_copy() -> Result<(), Box<dyn Error>> {
        let directory = TempDir::new()?;
        let source_path = directory.path().join("source.gguf");
        let source_bytes = valid_gguf()?;
        fs::write(&source_path, &source_bytes)?;
        let manager = ModelImportManager::initialize(directory.path().join("models"))?;
        let mut snapshots = Vec::new();

        let imported = manager.import(&source_path, |snapshot| snapshots.push(snapshot))?;

        assert_eq!(fs::read(&source_path)?, source_bytes);
        assert_ne!(imported.path(), source_path);
        assert_eq!(fs::read(imported.path())?, source_bytes);
        assert_eq!(imported.metadata().architecture(), "muse-glimmer");
        assert_eq!(imported.metadata().name(), "Imported Glimmer");
        assert_eq!(imported.sha256().len(), 64);
        assert!(manager.is_published(imported.path(), imported.sha256(), imported.byte_size()));
        assert!(!manager.is_published(imported.path(), &"0".repeat(64), imported.byte_size()));
        assert_eq!(snapshots.first().map(|item| item.copied_bytes()), Some(0));
        assert_eq!(snapshots.last().map(|item| item.remaining_bytes()), Some(0));
        Ok(())
    }

    #[test]
    fn unsupported_source_is_rejected_before_managed_copy() -> Result<(), Box<dyn Error>> {
        let directory = TempDir::new()?;
        let source_path = directory.path().join("source.gguf");
        let mut bytes = valid_gguf()?;
        let needle = b"muse-glimmer";
        let position = bytes
            .windows(needle.len())
            .position(|window| window == needle)
            .ok_or_else(|| io::Error::other("architecture test fixture missing"))?;
        bytes[position..position + needle.len()].copy_from_slice(b"llama_______");
        fs::write(&source_path, bytes)?;
        let root = directory.path().join("models");
        let manager = ModelImportManager::initialize(&root)?;

        assert!(matches!(
            manager.import(&source_path, |_| {}),
            Err(ImportError::Gguf(GgufError::UnsupportedArchitecture(_)))
        ));
        assert_eq!(fs::read_dir(root.join("installed"))?.count(), 0);
        assert_eq!(fs::read_dir(root.join("importing"))?.count(), 0);
        Ok(())
    }

    #[test]
    fn stale_candidates_are_removed_on_restart() -> Result<(), Box<dyn Error>> {
        let directory = TempDir::new()?;
        let root = directory.path().join("models");
        let importing = root.join("importing");
        fs::create_dir_all(&importing)?;
        let stale = importing.join("import-deadbeef.part");
        fs::write(&stale, b"partial")?;

        let _manager = ModelImportManager::initialize(&root)?;
        assert!(!stale.exists());
        Ok(())
    }
}
