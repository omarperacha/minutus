//! Handle-relative encrypted-only persistence for private password-vault namespaces.

use super::{
    MAX_PASSWORD_VAULT_BYTES, PasswordVaultDocument, PasswordVaultError as Error, PasswordVaultJob,
};
use crate::{MAX_LOCAL_VAULTS, VaultDeletionCommit, VaultKeyId, VaultSession};
use fs2::FileExt as _;
use rustix::{
    fd::OwnedFd,
    fs::{self, AtFlags, FileType, Mode, OFlags},
    io::Errno,
};
use std::{
    fmt::Write as _,
    fs::File,
    io::{Read as _, Write as _},
    path::{Component, Path},
};

const SUFFIX: &str = ".password-vault.json";
const FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::CLOEXEC)
    .union(OFlags::NOFOLLOW);
const MAX_STORE_ENTRIES: usize = 256;

fn ancestor_flags() -> OFlags {
    // App Sandbox permits traversal to its container, but does not grant directory
    // listing on ancestors such as /Users. O_SEARCH requests only traversal.
    // rustix does not expose Apple's O_SEARCH as a named flag; libc supplies the
    // SDK value without introducing an unsafe call. Final storage stays readable.
    #[cfg(target_os = "macos")]
    let flags = OFlags::from_bits_retain(libc::O_SEARCH as u32);
    #[cfg(not(target_os = "macos"))]
    let flags = OFlags::RDONLY | OFlags::DIRECTORY;
    flags | OFlags::CLOEXEC | OFlags::NOFOLLOW
}

/// Unix private directory capability. No wallet operation accepts a model path.
pub struct EncryptedDocumentStore<D: StoredPasswordDocument> {
    directory: OwnedFd,
    document: std::marker::PhantomData<D>,
}

/// Private storage of version-2 prototype wallets.
pub type PasswordVaultStore = EncryptedDocumentStore<PasswordVaultDocument>;

/// Sealed encrypted document contract for the private atomic store.
/// Implementations contain only validated encrypted material.
pub trait StoredPasswordDocument: sealed::Sealed + Clone + Eq {
    /// Maximum canonical encrypted document size.
    const MAX_BYTES: usize;
    /// Decode a bounded encrypted document.
    /// # Errors
    /// Rejects malformed or unsupported documents.
    fn decode(bytes: &[u8]) -> Result<Self, Error>;
    /// Encode canonical encrypted material.
    /// # Errors
    /// Rejects invalid document bounds.
    fn encode(&self) -> Result<Vec<u8>, Error>;
    /// Stable document identity.
    fn identity(&self) -> &VaultKeyId;
}
pub(crate) mod sealed {
    pub trait Sealed {}
}
impl sealed::Sealed for PasswordVaultDocument {}
impl StoredPasswordDocument for PasswordVaultDocument {
    const MAX_BYTES: usize = MAX_PASSWORD_VAULT_BYTES;
    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        Self::from_json(bytes)
    }
    fn encode(&self) -> Result<Vec<u8>, Error> {
        self.to_json()
    }
    fn identity(&self) -> &VaultKeyId {
        self.id()
    }
}

impl<D: StoredPasswordDocument> EncryptedDocumentStore<D> {
    /// Pins an absolute, no-symlink directory and cleans only owned encrypted stages
    /// under an exclusive lock. Only the final directory may be created.
    ///
    /// # Errors
    /// Rejects unsafe ancestors, owner/mode, malformed stages, contention and I/O.
    pub fn initialize(path: &Path) -> Result<Self, Error> {
        if !path.is_absolute() {
            return Err(Error::UnsafeStorage);
        }
        let components: Vec<_> = path.components().collect();
        if components.len() < 2 {
            return Err(Error::UnsafeStorage);
        }
        let mut directory = fs::open("/", ancestor_flags(), Mode::empty()).map_err(map_io)?;
        for (index, component) in components.iter().enumerate().skip(1) {
            let Component::Normal(name) = component else {
                return Err(Error::UnsafeStorage);
            };
            let final_component = index == components.len() - 1;
            let flags = if final_component {
                FLAGS | OFlags::DIRECTORY
            } else {
                ancestor_flags()
            };
            let opened = fs::openat(&directory, *name, flags, Mode::empty());
            directory = match opened {
                Ok(fd) => fd,
                Err(Errno::NOENT) if final_component => {
                    // Reopen the pinned immediate parent for durability: search-only
                    // descriptors do not support fsync. No ancestor listing is needed.
                    let parent =
                        fs::openat(&directory, ".", FLAGS | OFlags::DIRECTORY, Mode::empty())
                            .map_err(map_io)?;
                    fs::mkdirat(&parent, *name, Mode::from_bits_truncate(0o700)).map_err(map_io)?;
                    fs::fsync(&parent).map_err(map_io)?;
                    fs::openat(&parent, *name, FLAGS | OFlags::DIRECTORY, Mode::empty())
                        .map_err(map_io)?
                }
                Err(error) => return Err(map_io(error)),
            };
        }
        check_directory(&directory)?;
        let store = Self {
            directory,
            document: std::marker::PhantomData,
        };
        let _lock = store.lock()?;
        for name in store.entries()? {
            if let Some(hex) = name.strip_prefix(".stage-pw-") {
                if hex.len() != 32 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err(Error::UnsafeStorage);
                }
                let fd =
                    fs::openat(&store.directory, &name, FLAGS, Mode::empty()).map_err(map_io)?;
                let stage_stat = validate_file(&fd, 2, D::MAX_BYTES)?;
                if stage_stat.st_nlink == 2 {
                    // A crash between exclusive link publication and stage
                    // unlink leaves two names for the same encrypted inode.
                    // Only remove that second link after proving its exact target.
                    let mut bytes = Vec::new();
                    File::from(fd)
                        .take((D::MAX_BYTES + 1) as u64)
                        .read_to_end(&mut bytes)
                        .map_err(|_| Error::StorageUnavailable)?;
                    let document = D::decode(&bytes)?;
                    let live = fs::openat(
                        &store.directory,
                        filename(document.identity()),
                        FLAGS,
                        Mode::empty(),
                    )
                    .map_err(map_io)?;
                    let live_stat = validate_file(&live, 2, D::MAX_BYTES)?;
                    if live_stat.st_ino != stage_stat.st_ino
                        || live_stat.st_dev != stage_stat.st_dev
                    {
                        return Err(Error::UnsafeStorage);
                    }
                }
                fs::unlinkat(&store.directory, &name, AtFlags::empty()).map_err(map_io)?;
            }
        }
        fs::fsync(&store.directory).map_err(map_io)?;
        Ok(store)
    }

    /// Returns bounded stable identifiers from this test namespace only.
    ///
    /// # Errors
    /// Rejects unsafe relevant entries or an excessive inventory.
    pub fn vault_ids(&self) -> Result<Vec<VaultKeyId>, Error> {
        let _lock = self.lock()?;
        let mut ids = Vec::new();
        for name in self.entries()? {
            if let Some(id) = name.strip_suffix(SUFFIX) {
                let id = VaultKeyId::new(id).map_err(|_| Error::UnsafeStorage)?;
                let document = self.load_inner(&id)?;
                if document.identity() != &id {
                    return Err(Error::InvalidDocument);
                }
                ids.push(id);
            }
        }
        if ids.len() > MAX_LOCAL_VAULTS {
            return Err(Error::InvalidDocument);
        }
        ids.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        Ok(ids)
    }

    /// Reads at most the format bound from an owned, regular, private file.
    ///
    /// # Errors
    /// Returns not-found, strict format/old-version, path, mode or I/O failures.
    pub fn load(&self, id: &VaultKeyId) -> Result<D, Error> {
        let _lock = self.lock()?;
        self.load_inner(id)
    }

    /// Holds exclusive revision ownership across a bounded native operation.
    /// Used to prevent a concurrent password/wallet replacement during signing
    /// or between fresh authentication and session installation.
    ///
    /// # Errors
    /// Rejects contention, changed/unsafe storage and the operation's error.
    pub fn with_revision<T>(
        &self,
        expected: &D,
        operation: impl FnOnce() -> Result<T, Error>,
    ) -> Result<T, Error> {
        let _lock = self.lock()?;
        self.require_revision(expected)?;
        operation()
    }

    /// Creates an encrypted file without overwriting any existing identifier.
    ///
    /// # Errors
    /// Returns cancellation, conflict, storage or uncertain-publication failures.
    pub fn create(&self, document: &D, job: &PasswordVaultJob) -> Result<(), Error> {
        self.publish(document, None, job, &NoFaults)
    }

    /// Creates the prototype's single wallet only while the store is empty.
    /// The empty-slot check and publication share exclusive ownership.
    ///
    /// # Errors
    /// Rejects a concurrent creation, stale work or unsafe/unavailable storage.
    pub fn create_in_empty_store(&self, document: &D, job: &PasswordVaultJob) -> Result<(), Error> {
        let _lock = self.lock()?;
        if self.entries()?.iter().any(|name| name.ends_with(SUFFIX)) {
            return Err(Error::Conflict);
        }
        self.publish_locked(document, None, job, &NoFaults)
    }

    pub(crate) fn replace_record(
        &self,
        previous: &D,
        replacement: &D,
        job: &PasswordVaultJob,
    ) -> Result<(), Error> {
        if previous.identity() != replacement.identity() {
            return Err(Error::Conflict);
        }
        self.publish(replacement, Some(previous), job, &NoFaults)
    }

    pub(crate) fn remove_record(&self, expected: &D, job: &PasswordVaultJob) -> Result<(), Error> {
        self.remove_record_with_faults(expected, job, &NoFaults)
    }
    fn remove_record_with_faults(
        &self,
        expected: &D,
        job: &PasswordVaultJob,
        faults: &impl Faults,
    ) -> Result<(), Error> {
        let _lock = self.lock()?;
        let mut removed = false;
        let result = job.with_current(|| {
            self.require_revision(expected)?;
            faults.at(Phase::Publish)?;
            fs::unlinkat(
                &self.directory,
                filename(expected.identity()),
                AtFlags::empty(),
            )
            .map_err(map_io)?;
            removed = true;
            faults
                .at(Phase::DirectorySync)
                .map_err(|_| Error::CommitUncertain)?;
            fs::fsync(&self.directory).map_err(|_| Error::CommitUncertain)
        });
        if removed && result == Err(Error::Cancelled) {
            Err(Error::CommitUncertain)
        } else {
            result
        }
    }

    fn publish(
        &self,
        document: &D,
        previous: Option<&D>,
        job: &PasswordVaultJob,
        faults: &impl Faults,
    ) -> Result<(), Error> {
        let _lock = self.lock()?;
        self.publish_locked(document, previous, job, faults)
    }

    // Callers hold the store lock through staging, checks and publication.
    fn publish_locked(
        &self,
        document: &D,
        previous: Option<&D>,
        job: &PasswordVaultJob,
        faults: &impl Faults,
    ) -> Result<(), Error> {
        job.check()?;
        if previous.is_none()
            && self
                .entries()?
                .iter()
                .filter(|n| n.ends_with(SUFFIX))
                .count()
                >= MAX_LOCAL_VAULTS
        {
            return Err(Error::Conflict);
        }
        if let Some(previous) = previous {
            self.require_revision(previous)?;
        }
        let bytes = document.encode()?;
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random).map_err(|_| Error::RandomnessUnavailable)?;
        let mut name = String::from(".stage-pw-");
        for byte in random {
            let _ = write!(name, "{byte:02x}");
        }
        let fd = fs::openat(
            &self.directory,
            &name,
            OFlags::CREATE | OFlags::EXCL | OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::from_bits_truncate(0o600),
        )
        .map_err(map_io)?;
        let _stage = Stage {
            directory: &self.directory,
            name: &name,
        };
        let mut file = File::from(fd);
        check_file(&file, D::MAX_BYTES)?;
        faults.at(Phase::Write)?;
        file.write_all(&bytes)
            .map_err(|_| Error::StorageUnavailable)?;
        faults.at(Phase::FileSync)?;
        file.sync_all().map_err(|_| Error::StorageUnavailable)?;
        faults.at(Phase::Reopen)?;
        if self.read_name(&name)? != *document {
            return Err(Error::InvalidDocument);
        }
        let mut published = false;
        let result = job.with_current(|| {
            if let Some(previous) = previous {
                self.require_revision(previous)?;
            }
            faults.at(Phase::Publish)?;
            if previous.is_some() {
                fs::renameat(
                    &self.directory,
                    &name,
                    &self.directory,
                    filename(document.identity()),
                )
                .map_err(map_io)?;
            } else {
                fs::linkat(
                    &self.directory,
                    &name,
                    &self.directory,
                    filename(document.identity()),
                    AtFlags::empty(),
                )
                .map_err(map_io)?;
            }
            published = true;
            // A failed sync after publication cannot justify rolling back a
            // password change to an old-password route. Caller must lock/reopen.
            faults
                .at(Phase::DirectorySync)
                .map_err(|_| Error::CommitUncertain)?;
            fs::fsync(&self.directory).map_err(|_| Error::CommitUncertain)?;
            Ok(())
        });
        if published && result == Err(Error::Cancelled) {
            Err(Error::CommitUncertain)
        } else {
            result
        }
    }

    fn require_revision(&self, previous: &D) -> Result<(), Error> {
        if self.load_inner(previous.identity())?.encode()? != previous.encode()? {
            return Err(Error::Conflict);
        }
        Ok(())
    }
    fn load_inner(&self, id: &VaultKeyId) -> Result<D, Error> {
        let document = self.read_name(&filename(id))?;
        if document.identity() != id {
            return Err(Error::InvalidDocument);
        }
        Ok(document)
    }
    fn read_name(&self, name: &str) -> Result<D, Error> {
        let fd = fs::openat(&self.directory, name, FLAGS, Mode::empty()).map_err(map_io)?;
        check_file(&fd, D::MAX_BYTES)?;
        let mut bytes = Vec::new();
        File::from(fd)
            .take((D::MAX_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::StorageUnavailable)?;
        D::decode(&bytes)
    }
    fn lock(&self) -> Result<File, Error> {
        check_directory(&self.directory)?;
        let fd = fs::openat(
            &self.directory,
            ".operation.lock",
            OFlags::CREATE | OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::from_bits_truncate(0o600),
        )
        .map_err(map_io)?;
        check_file(&fd, D::MAX_BYTES)?;
        let file = File::from(fd);
        file.try_lock_exclusive().map_err(|_| Error::Busy)?;
        Ok(file)
    }
    fn entries(&self) -> Result<Vec<String>, Error> {
        let mut result = Vec::new();
        let mut entries = fs::Dir::read_from(&self.directory).map_err(map_io)?;
        for entry in &mut entries {
            let entry = entry.map_err(map_io)?;
            let name = entry
                .file_name()
                .to_str()
                .map_err(|_| Error::UnsafeStorage)?;
            if name == "." || name == ".." {
                continue;
            }
            result.push(name.to_owned());
            if result.len() > MAX_STORE_ENTRIES {
                return Err(Error::UnsafeStorage);
            }
        }
        Ok(result)
    }
}

impl PasswordVaultStore {
    /// Replaces only the exact retained revision, with an identical secret payload.
    ///
    /// # Errors
    /// Rejects changed wallet/payload/revision, stale work and unsafe storage.
    pub fn replace(
        &self,
        previous: &PasswordVaultDocument,
        replacement: &PasswordVaultDocument,
        job: &PasswordVaultJob,
    ) -> Result<(), Error> {
        if previous.id() != replacement.id() || previous.payload() != replacement.payload() {
            return Err(Error::Conflict);
        }
        self.publish(replacement, Some(previous), job, &NoFaults)
    }

    /// Consumes the password-specific native deletion confirmation and locks the
    /// matching session before unlinking encrypted material. External copies and
    /// recovery material remain; no cryptographic/physical erasure is claimed.
    ///
    /// # Errors
    /// Rejects wrong protection/session, cancellation and incomplete removal.
    pub fn delete(
        &self,
        expected: &PasswordVaultDocument,
        commit: VaultDeletionCommit,
        session: &mut VaultSession,
        job: &PasswordVaultJob,
    ) -> Result<(), Error> {
        let id = commit
            .authorize_password_removal(session)
            .map_err(|_| Error::AuthenticationFailed)?;
        if &id != expected.id() {
            return Err(Error::Conflict);
        }
        self.remove_record(expected, job)
    }
}

struct Stage<'a> {
    directory: &'a OwnedFd,
    name: &'a str,
}
impl Drop for Stage<'_> {
    fn drop(&mut self) {
        let _ = fs::unlinkat(self.directory, self.name, AtFlags::empty());
        let _ = fs::fsync(self.directory);
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Write,
    FileSync,
    Reopen,
    Publish,
    DirectorySync,
}
trait Faults {
    fn at(&self, phase: Phase) -> Result<(), Error>;
}
struct NoFaults;
impl Faults for NoFaults {
    fn at(&self, _: Phase) -> Result<(), Error> {
        Ok(())
    }
}
fn filename(id: &VaultKeyId) -> String {
    format!("{}{SUFFIX}", id.as_str())
}
fn check_file(fd: impl rustix::fd::AsFd, maximum_bytes: usize) -> Result<(), Error> {
    validate_file(fd, 1, maximum_bytes).map(|_| ())
}
fn validate_file(
    fd: impl rustix::fd::AsFd,
    maximum_links: u64,
    maximum_bytes: usize,
) -> Result<fs::Stat, Error> {
    let stat = fs::fstat(fd).map_err(map_io)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_nlink == 0
        || u64::from(stat.st_nlink) > maximum_links
        || stat.st_uid != rustix::process::geteuid().as_raw()
        || stat.st_mode & 0o777 != 0o600
        || stat.st_size < 0
        || stat.st_size > i64::try_from(maximum_bytes).map_err(|_| Error::UnsafeStorage)?
    {
        return Err(Error::UnsafeStorage);
    }
    Ok(stat)
}
fn check_directory(fd: &OwnedFd) -> Result<(), Error> {
    let stat = fs::fstat(fd).map_err(map_io)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory
        || stat.st_uid != rustix::process::geteuid().as_raw()
        || stat.st_mode & 0o777 != 0o700
        || stat.st_nlink == 0
    {
        return Err(Error::UnsafeStorage);
    }
    Ok(())
}
fn map_io(error: Errno) -> Error {
    match error {
        Errno::NOENT => Error::NotFound,
        Errno::EXIST => Error::Conflict,
        Errno::LOOP | Errno::NOTDIR => Error::UnsafeStorage,
        _ => Error::StorageUnavailable,
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "storage/revocation_tests.rs"]
mod revocation_tests;

#[cfg(all(test, target_os = "macos"))]
#[path = "storage/sandbox_tests.rs"]
mod sandbox_tests;
