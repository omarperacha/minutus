//! Private append-safe storage for encrypted wallet documents.

use std::{
    error::Error,
    fmt, fs,
    io::Write as _,
    path::{Path, PathBuf},
};

use crate::{MAX_LOCAL_VAULTS, VaultDocument, VaultKeyId};

const VAULT_SUFFIX: &str = ".vault.json";

/// Closed encrypted-vault filesystem failures.
#[derive(Debug)]
pub enum VaultStoreError {
    /// The private store could not be prepared or accessed.
    Io(std::io::Error),
    /// A persisted document was malformed or unauthenticated metadata was invalid.
    InvalidDocument,
    /// The selected vault does not exist.
    NotFound,
    /// A new vault cannot overwrite an existing identifier.
    AlreadyExists,
    /// The bounded local vault inventory is full.
    InventoryFull,
}

impl fmt::Display for VaultStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Io(_) => "encrypted wallet storage is unavailable",
            Self::InvalidDocument => "encrypted wallet document is invalid",
            Self::NotFound => "encrypted wallet was not found",
            Self::AlreadyExists => "encrypted wallet already exists",
            Self::InventoryFull => "local wallet inventory is full",
        })
    }
}

impl Error for VaultStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for VaultStoreError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Wallet-owned private storage containing encrypted documents only.
pub struct EncryptedVaultStore {
    directory: PathBuf,
}

impl EncryptedVaultStore {
    /// Initializes a private directory and removes abandoned encrypted staging files.
    ///
    /// # Errors
    ///
    /// Returns a storage error when directory preparation, permission hardening
    /// or abandoned-stage cleanup fails.
    pub fn initialize(directory: impl Into<PathBuf>) -> Result<Self, VaultStoreError> {
        let directory = directory.into();
        fs::create_dir_all(&directory)?;
        restrict_directory(&directory)?;
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            if entry.file_type()?.is_file()
                && entry.file_name().to_string_lossy().starts_with(".stage-")
            {
                fs::remove_file(entry.path())?;
            }
        }
        Ok(Self { directory })
    }

    /// Lists canonical local vault identifiers in stable order.
    ///
    /// # Errors
    ///
    /// Returns a storage or validation error for unreadable entries, malformed
    /// identifiers or an inventory exceeding the reviewed bound.
    pub fn vault_ids(&self) -> Result<Vec<VaultKeyId>, VaultStoreError> {
        let mut ids = Vec::new();
        for entry in fs::read_dir(&self.directory)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_str().ok_or(VaultStoreError::InvalidDocument)?;
            let Some(id) = name.strip_suffix(VAULT_SUFFIX) else {
                continue;
            };
            ids.push(VaultKeyId::new(id).map_err(|_| VaultStoreError::InvalidDocument)?);
        }
        ids.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        if ids.len() > MAX_LOCAL_VAULTS {
            return Err(VaultStoreError::InvalidDocument);
        }
        Ok(ids)
    }

    /// Loads and strictly validates one encrypted document.
    ///
    /// # Errors
    ///
    /// Returns not-found, storage or strict document-validation failures.
    pub fn load(&self, id: &VaultKeyId) -> Result<VaultDocument, VaultStoreError> {
        let path = self.path(id);
        let bytes = fs::read(path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                VaultStoreError::NotFound
            } else {
                VaultStoreError::Io(error)
            }
        })?;
        VaultDocument::from_json(&bytes).map_err(|_| VaultStoreError::InvalidDocument)
    }

    /// Removes one encrypted document without touching its separate Keychain key.
    ///
    /// # Errors
    ///
    /// Returns not-found or storage failures when removal cannot be completed
    /// and synchronized.
    pub fn remove(&self, id: &VaultKeyId) -> Result<(), VaultStoreError> {
        fs::remove_file(self.path(id)).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                VaultStoreError::NotFound
            } else {
                VaultStoreError::Io(error)
            }
        })?;
        sync_parent(&self.path(id))?;
        Ok(())
    }

    /// Writes and synchronizes a new encrypted staging document.
    ///
    /// # Errors
    ///
    /// Returns a closed error for a full inventory, existing destination,
    /// invalid serialization, unavailable randomness or storage I/O.
    pub fn prepare_create(
        &self,
        id: &VaultKeyId,
        document: &VaultDocument,
    ) -> Result<PreparedVaultWrite, VaultStoreError> {
        if self.vault_ids()?.len() >= MAX_LOCAL_VAULTS {
            return Err(VaultStoreError::InventoryFull);
        }
        let destination = self.path(id);
        if destination.exists() {
            return Err(VaultStoreError::AlreadyExists);
        }
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random).map_err(|_| {
            VaultStoreError::Io(std::io::Error::other(
                "vault staging randomness unavailable",
            ))
        })?;
        let stage = self
            .directory
            .join(format!(".stage-{}", encode_hex(&random)));
        let mut file = private_create_new(&stage)?;
        file.write_all(
            &document
                .to_json()
                .map_err(|_| VaultStoreError::InvalidDocument)?,
        )?;
        file.sync_all()?;
        Ok(PreparedVaultWrite {
            stage,
            destination,
            committed: false,
        })
    }

    fn path(&self, id: &VaultKeyId) -> PathBuf {
        self.directory
            .join(format!("{}{VAULT_SUFFIX}", id.as_str()))
    }
}

/// Synchronized encrypted staging file awaiting Keychain commit.
pub struct PreparedVaultWrite {
    stage: PathBuf,
    destination: PathBuf,
    committed: bool,
}

impl PreparedVaultWrite {
    /// Publishes the staged encrypted document without overwriting another vault.
    ///
    /// # Errors
    ///
    /// Returns an existing-destination or storage synchronization error. A
    /// failed publication cleans up its unpublished staging file on drop.
    pub fn commit(mut self) -> Result<(), VaultStoreError> {
        fs::hard_link(&self.stage, &self.destination).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                VaultStoreError::AlreadyExists
            } else {
                VaultStoreError::Io(error)
            }
        })?;
        if let Err(error) = sync_parent(&self.destination) {
            let _ = fs::remove_file(&self.destination);
            return Err(VaultStoreError::Io(error));
        }
        self.committed = true;
        let _ = fs::remove_file(&self.stage);
        Ok(())
    }
}

impl Drop for PreparedVaultWrite {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.stage);
        }
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    use fmt::Write as _;
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn sync_parent(path: &Path) -> Result<(), std::io::Error> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("vault path has no parent"))?;
    fs::File::open(parent)?.sync_all()
}

#[cfg(unix)]
fn private_create_new(path: &Path) -> Result<fs::File, std::io::Error> {
    use std::os::unix::fs::OpenOptionsExt as _;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn private_create_new(path: &Path) -> Result<fs::File, std::io::Error> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

#[cfg(unix)]
fn restrict_directory(path: &Path) -> Result<(), std::io::Error> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn restrict_directory(_path: &Path) -> Result<(), std::io::Error> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AccountKeyOrigin, AccountMetadata};

    #[test]
    fn new_vault_is_private_atomic_and_never_overwritten() -> Result<(), Box<dyn Error>> {
        let temporary = tempfile::TempDir::new()?;
        let store = EncryptedVaultStore::initialize(temporary.path())?;
        let id = VaultKeyId::new("vault-01")?;
        let document = VaultDocument::new(
            [1; 24],
            [2; 32],
            vec![3],
            [4; 16],
            vec![AccountMetadata::new(
                "account-01",
                "0x0000000000000000000000000000000000000001",
                8453,
                AccountKeyOrigin::ImportedPrivateKey,
            )?],
        )?;
        store.prepare_create(&id, &document)?.commit()?;
        assert_eq!(store.load(&id)?, document);
        assert_eq!(store.vault_ids()?, vec![id.clone()]);
        assert!(matches!(
            store.prepare_create(&id, &document),
            Err(VaultStoreError::AlreadyExists)
        ));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                fs::metadata(temporary.path())?.permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(store.path(&id))?.permissions().mode() & 0o777,
                0o600
            );
        }
        Ok(())
    }
}
