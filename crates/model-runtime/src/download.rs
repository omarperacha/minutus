//! Resumable, integrity-checked storage for catalogue model artifacts.

use std::{
    error::Error,
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use reqwest::{
    StatusCode,
    blocking::Client,
    header::{ACCEPT_ENCODING, CONTENT_LENGTH, CONTENT_RANGE, RANGE},
    redirect,
};
use sha2::{Digest, Sha256};
use url::Url;

use crate::{ArtifactKey, ModelArtifact, blocking_client::BlockingClient};

const COPY_BUFFER_BYTES: usize = 1024 * 1024;
const PROGRESS_INTERVAL_BYTES: u64 = 16 * 1024 * 1024;
const MAX_REDIRECTS: usize = 10;

/// Current native phase of a catalogue model installation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DownloadPhase {
    /// Bytes are being fetched or resumed into private partial storage.
    Downloading,
    /// The complete candidate is being hashed against the catalogue digest.
    Verifying,
    /// Verified bytes are being atomically published into managed storage.
    Installing,
}

/// Snapshot emitted while a model artifact is being downloaded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DownloadProgress {
    downloaded_bytes: u64,
    total_bytes: u64,
    resumed: bool,
    phase: DownloadPhase,
}

impl DownloadProgress {
    pub(crate) const fn provider(
        downloaded_bytes: u64,
        total_bytes: u64,
        phase: DownloadPhase,
    ) -> Self {
        Self {
            downloaded_bytes,
            total_bytes,
            resumed: false,
            phase,
        }
    }

    /// Returns bytes present in the verified download candidate so far.
    #[must_use]
    pub const fn downloaded_bytes(self) -> u64 {
        self.downloaded_bytes
    }

    /// Returns the expected complete artifact size.
    #[must_use]
    pub const fn total_bytes(self) -> u64 {
        self.total_bytes
    }

    /// Returns bytes still required before integrity verification can begin.
    #[must_use]
    pub const fn remaining_bytes(self) -> u64 {
        self.total_bytes.saturating_sub(self.downloaded_bytes)
    }

    /// Reports whether this attempt started with recoverable partial bytes.
    #[must_use]
    pub const fn resumed(self) -> bool {
        self.resumed
    }

    /// Returns the native download, verification or installation phase.
    #[must_use]
    pub const fn phase(self) -> DownloadPhase {
        self.phase
    }
}

/// Cooperative cancellation handle for one model download.
#[derive(Clone, Debug, Default)]
pub struct DownloadCancellation {
    cancelled: Arc<AtomicBool>,
}

impl DownloadCancellation {
    /// Creates a cancellation handle in the running state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation. Already downloaded bytes remain resumable.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Reports whether cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

/// A model artifact published only after its expected digest was verified.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedModelArtifact {
    key: ArtifactKey,
    path: PathBuf,
    byte_size: u64,
    sha256: String,
}

impl VerifiedModelArtifact {
    /// Returns the catalogue identity of the installed artifact.
    #[must_use]
    pub const fn key(&self) -> &ArtifactKey {
        &self.key
    }

    /// Returns the Rust-managed installed path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the verified file length.
    #[must_use]
    pub const fn byte_size(&self) -> u64 {
        self.byte_size
    }

    /// Returns the verified lowercase SHA-256 digest.
    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}

/// Rust-owned storage and HTTP client for local model downloads.
pub struct ModelDownloadManager {
    partial_directory: PathBuf,
    installed_directory: PathBuf,
    client: BlockingClient,
    operation: Mutex<()>,
}

impl ModelDownloadManager {
    /// Creates private sibling directories for partial and installed artifacts.
    ///
    /// # Errors
    ///
    /// Returns an error if storage cannot be created securely or the HTTPS
    /// client cannot be initialized.
    pub fn initialize(root: impl AsRef<Path>) -> Result<Self, DownloadError> {
        let root = root.as_ref();
        create_private_directory(root)?;
        let root = root.canonicalize().map_err(DownloadError::Storage)?;
        let partial_directory = root.join("partial");
        let installed_directory = root.join("installed");
        create_private_directory(&partial_directory)?;
        create_private_directory(&installed_directory)?;
        let client = BlockingClient::build(
            Client::builder()
                .connect_timeout(Duration::from_secs(30))
                .redirect(redirect::Policy::custom(|attempt| {
                    if attempt.url().scheme() != "https" {
                        attempt.error("model download redirect did not use HTTPS")
                    } else if attempt.previous().len() >= MAX_REDIRECTS {
                        attempt.error("model download exceeded its redirect limit")
                    } else {
                        attempt.follow()
                    }
                }))
                .user_agent("Minutus/0.1 local-model-downloader"),
        )
        .map_err(|()| DownloadError::Unavailable)?;
        Ok(Self {
            partial_directory,
            installed_directory,
            client,
            operation: Mutex::new(()),
        })
    }

    /// Downloads, resumes, verifies and atomically publishes one catalogue artifact.
    ///
    /// The callback receives an initial snapshot and subsequent byte progress.
    /// Only a `VerifiedModelArtifact` returned by this method is eligible for
    /// persistence or inference.
    ///
    /// # Errors
    ///
    /// Returns a typed error for storage, disk-space, protocol, cancellation,
    /// transfer or integrity failures. Partial bytes survive recoverable failures.
    pub fn download(
        &self,
        artifact: &ModelArtifact,
        cancellation: &DownloadCancellation,
        progress: impl FnMut(DownloadProgress),
    ) -> Result<VerifiedModelArtifact, DownloadError> {
        let source = ReqwestSource {
            client: &self.client,
        };
        self.download_with(artifact, cancellation, progress, &source, |path| {
            fs2::available_space(path)
        })
    }

    /// Checks disk space for the aggregate missing bytes of a selected profile.
    ///
    /// Existing published artifacts and retained partial lengths are deducted;
    /// malformed oversized partials are treated as requiring a full download.
    ///
    /// # Errors
    ///
    /// Returns an error when size aggregation overflows, storage cannot be
    /// inspected or available disk space is below the aggregate requirement.
    pub fn ensure_disk_space_for(
        &self,
        artifacts: &[&ModelArtifact],
    ) -> Result<u64, DownloadError> {
        self.ensure_disk_space_for_with(artifacts, |path| fs2::available_space(path))
    }

    /// Reports whether metadata points to this manager's published artifact.
    ///
    /// This is a cheap presentation-state check. Inference must still perform
    /// complete digest verification before loading the file.
    #[must_use]
    pub fn is_published(&self, artifact: &ModelArtifact, path: &Path) -> bool {
        if path != self.installed_path(artifact) {
            return false;
        }
        fs::symlink_metadata(path).is_ok_and(|metadata| {
            metadata.file_type().is_file() && metadata.len() == artifact.installed_bytes()
        })
    }

    fn download_with<S, F>(
        &self,
        artifact: &ModelArtifact,
        cancellation: &DownloadCancellation,
        mut progress: impl FnMut(DownloadProgress),
        source: &S,
        available_space: F,
    ) -> Result<VerifiedModelArtifact, DownloadError>
    where
        S: DownloadSource,
        F: Fn(&Path) -> io::Result<u64>,
    {
        let _operation = self
            .operation
            .lock()
            .map_err(|_| DownloadError::Unavailable)?;
        if artifact.download_bytes() != artifact.installed_bytes() {
            return Err(DownloadError::UnsupportedInstallationLayout);
        }
        let partial_path = self.partial_path(artifact);
        let installed_path = self.installed_path(artifact);

        if installed_path.exists() {
            progress(DownloadProgress {
                downloaded_bytes: artifact.download_bytes(),
                total_bytes: artifact.download_bytes(),
                resumed: false,
                phase: DownloadPhase::Verifying,
            });
            check_cancelled(cancellation)?;
            let installed = verify_file(artifact, &installed_path, cancellation)?;
            remove_partial_if_present(&partial_path)?;
            progress(DownloadProgress {
                downloaded_bytes: artifact.download_bytes(),
                total_bytes: artifact.download_bytes(),
                resumed: false,
                phase: DownloadPhase::Installing,
            });
            return Ok(installed);
        }

        let mut partial = open_private_partial(&partial_path)?;
        let mut offset = partial.metadata().map_err(DownloadError::Storage)?.len();
        if offset > artifact.download_bytes() {
            reset_partial(&mut partial)?;
            offset = 0;
        }
        let resumed = offset > 0;
        ensure_disk_space(&self.partial_directory, artifact, offset, &available_space)?;
        progress(DownloadProgress {
            downloaded_bytes: offset,
            total_bytes: artifact.download_bytes(),
            resumed,
            phase: DownloadPhase::Downloading,
        });
        check_cancelled(cancellation)?;

        if offset == artifact.download_bytes() {
            return self.verify_and_publish(
                artifact,
                partial,
                &partial_path,
                installed_path,
                resumed,
                cancellation,
                &mut progress,
            );
        }

        let response = source.open(artifact.download_url(), offset);
        check_cancelled(cancellation)?;
        let response = response?;
        let downloaded = transfer_response(
            artifact,
            cancellation,
            &mut progress,
            &mut partial,
            response,
            &mut offset,
            resumed,
        )?;
        partial.sync_all().map_err(DownloadError::Storage)?;
        if downloaded != artifact.download_bytes() {
            return Err(DownloadError::Incomplete {
                downloaded,
                expected: artifact.download_bytes(),
            });
        }
        check_cancelled(cancellation)?;
        self.verify_and_publish(
            artifact,
            partial,
            &partial_path,
            installed_path,
            resumed,
            cancellation,
            &mut progress,
        )
    }

    fn ensure_disk_space_for_with<F>(
        &self,
        artifacts: &[&ModelArtifact],
        available_space: F,
    ) -> Result<u64, DownloadError>
    where
        F: Fn(&Path) -> io::Result<u64>,
    {
        let mut required = 0_u64;
        for artifact in artifacts {
            let installed = self.installed_path(artifact);
            if fs::symlink_metadata(&installed).is_ok_and(|metadata| {
                metadata.file_type().is_file() && metadata.len() == artifact.installed_bytes()
            }) {
                continue;
            }
            let retained = fs::symlink_metadata(self.partial_path(artifact))
                .ok()
                .filter(|metadata| metadata.file_type().is_file())
                .map_or(0, |metadata| metadata.len());
            let retained = if retained <= artifact.download_bytes() {
                retained
            } else {
                0
            };
            required = required
                .checked_add(artifact.download_bytes().saturating_sub(retained))
                .ok_or(DownloadError::ArtifactTooLarge)?;
        }
        let available = available_space(&self.partial_directory).map_err(DownloadError::Storage)?;
        if available < required {
            return Err(DownloadError::InsufficientDiskSpace {
                required,
                available,
            });
        }
        Ok(required)
    }

    // Pass cancellation explicitly through publication rather than retain hidden
    // mutable request state on the shared download manager.
    #[allow(clippy::too_many_arguments)]
    fn verify_and_publish(
        &self,
        artifact: &ModelArtifact,
        mut partial: File,
        partial_path: &Path,
        installed_path: PathBuf,
        resumed: bool,
        cancellation: &DownloadCancellation,
        progress: &mut impl FnMut(DownloadProgress),
    ) -> Result<VerifiedModelArtifact, DownloadError> {
        progress(DownloadProgress {
            downloaded_bytes: artifact.download_bytes(),
            total_bytes: artifact.download_bytes(),
            resumed,
            phase: DownloadPhase::Verifying,
        });
        partial
            .seek(SeekFrom::Start(0))
            .map_err(DownloadError::Storage)?;
        let actual = sha256_reader(&mut partial, cancellation)?;
        if actual != artifact.sha256() {
            reset_partial(&mut partial)?;
            return Err(DownloadError::ChecksumMismatch {
                expected: artifact.sha256().to_owned(),
                actual,
            });
        }
        if let Err(error) = validate_format(artifact, &mut partial) {
            reset_partial(&mut partial)?;
            return Err(error);
        }
        partial.sync_all().map_err(DownloadError::Storage)?;
        progress(DownloadProgress {
            downloaded_bytes: artifact.download_bytes(),
            total_bytes: artifact.download_bytes(),
            resumed,
            phase: DownloadPhase::Installing,
        });
        check_cancelled(cancellation)?;
        drop(partial);
        fs::rename(partial_path, &installed_path).map_err(DownloadError::Storage)?;
        sync_directory(&self.installed_directory)?;
        Ok(VerifiedModelArtifact {
            key: artifact.key().clone(),
            path: installed_path,
            byte_size: artifact.download_bytes(),
            sha256: actual,
        })
    }

    fn partial_path(&self, artifact: &ModelArtifact) -> PathBuf {
        self.partial_directory
            .join(format!("{}.part", artifact.sha256()))
    }

    pub(crate) fn installed_path(&self, artifact: &ModelArtifact) -> PathBuf {
        let extension = match artifact.format() {
            crate::ArtifactFormat::Gguf => "gguf",
            crate::ArtifactFormat::Safetensors => "safetensors",
            crate::ArtifactFormat::WhisperGgml => "bin",
        };
        self.installed_directory
            .join(format!("{}.{extension}", artifact.sha256()))
    }

    pub(crate) fn verify_installed(
        &self,
        artifact: &ModelArtifact,
        cancellation: &DownloadCancellation,
    ) -> Result<VerifiedModelArtifact, DownloadError> {
        verify_file(artifact, &self.installed_path(artifact), cancellation)
    }

    pub(crate) fn remove_artifact(&self, artifact: &ModelArtifact) -> Result<(), DownloadError> {
        let _operation = self
            .operation
            .lock()
            .map_err(|_| DownloadError::Unavailable)?;
        remove_partial_if_present(&self.installed_path(artifact))?;
        remove_partial_if_present(&self.partial_path(artifact))?;
        sync_directory(&self.installed_directory)
    }
}

mod transport;

use transport::{
    DownloadSource, ReqwestSource, check_cancelled, create_private_directory, ensure_disk_space,
    open_private_partial, remove_partial_if_present, reset_partial, sha256_reader, sync_directory,
    transfer_response, validate_format, verify_file,
};
#[cfg(test)]
use transport::{ResponseKind, SourceResponse, parse_content_range};

/// Failure while downloading, verifying or publishing a model artifact.
#[derive(Debug)]
pub enum DownloadError {
    /// Model download storage could not be prepared or updated.
    Storage(io::Error),
    /// The HTTPS client or request failed.
    Network(reqwest::Error),
    /// Reading the response body failed after recoverable bytes were persisted.
    Transfer(io::Error),
    /// Another thread failed while holding the model download lock.
    Unavailable,
    /// The user cancelled the operation; partial state remains resumable.
    Cancelled,
    /// Available disk space cannot hold the remaining download and installation.
    InsufficientDiskSpace {
        /// Additional bytes needed before starting the transfer.
        required: u64,
        /// Bytes currently available to this application.
        available: u64,
    },
    /// The server returned a status other than 200 or 206.
    HttpStatus(u16),
    /// A partial response omitted its required `Content-Range` header.
    MissingContentRange,
    /// The server returned a malformed `Content-Range` header.
    InvalidContentRange,
    /// The server resumed from the wrong offset or reported the wrong total.
    InvalidRangeResponse {
        /// Byte offset requested by Rust.
        requested_start: u64,
        /// Byte offset returned by the server.
        received_start: u64,
        /// Total artifact size returned by the server.
        received_total: u64,
    },
    /// A declared response length did not match the expected remaining bytes.
    UnexpectedContentLength {
        /// Remaining bytes expected from the response.
        expected: u64,
        /// Bytes declared by the response.
        received: u64,
    },
    /// The response ended before the expected artifact size was reached.
    Incomplete {
        /// Bytes safely retained in partial storage.
        downloaded: u64,
        /// Complete expected artifact size.
        expected: u64,
    },
    /// The response exceeded the catalogue artifact size.
    ArtifactTooLarge,
    /// The artifact requires an installation transform this downloader does not support.
    UnsupportedInstallationLayout,
    /// The complete candidate did not match its catalogue SHA-256.
    ChecksumMismatch {
        /// Digest required by the validated catalogue.
        expected: String,
        /// Digest computed over the downloaded candidate.
        actual: String,
    },
    /// A previously published artifact no longer matches its catalogue entry.
    InstalledArtifactInvalid,
    /// The artifact header is incompatible with the selected speech format.
    IncompatibleArtifact,
}

impl fmt::Display for DownloadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(_) => formatter.write_str("model download storage is unavailable"),
            Self::Network(_) => formatter.write_str("the model download request failed"),
            Self::Transfer(_) => formatter.write_str("the model download was interrupted"),
            Self::Unavailable => formatter.write_str("model downloads are unavailable"),
            Self::Cancelled => formatter.write_str("the model download was cancelled"),
            Self::InsufficientDiskSpace {
                required,
                available,
            } => write!(
                formatter,
                "insufficient disk space for model download: {required} bytes required, {available} bytes available"
            ),
            Self::HttpStatus(status) => {
                write!(formatter, "model download server returned HTTP {status}")
            }
            Self::MissingContentRange => {
                formatter.write_str("model download resume response omitted Content-Range")
            }
            Self::InvalidContentRange => {
                formatter.write_str("model download server returned an invalid Content-Range")
            }
            Self::InvalidRangeResponse { .. } => {
                formatter.write_str("model download server returned the wrong byte range")
            }
            Self::UnexpectedContentLength { expected, received } => write!(
                formatter,
                "model download declared {received} bytes; {expected} bytes were expected"
            ),
            Self::Incomplete {
                downloaded,
                expected,
            } => write!(
                formatter,
                "model download stopped at {downloaded} of {expected} bytes"
            ),
            Self::ArtifactTooLarge => {
                formatter.write_str("model download exceeded its catalogue size")
            }
            Self::UnsupportedInstallationLayout => formatter.write_str(
                "model artifact cannot be installed directly by this application version",
            ),
            Self::ChecksumMismatch { .. } => {
                formatter.write_str("model download failed SHA-256 verification")
            }
            Self::IncompatibleArtifact => formatter.write_str(
                "the speech model is incompatible; remove it and download the supported profile",
            ),
            Self::InstalledArtifactInvalid => {
                formatter.write_str("installed model failed integrity verification")
            }
        }
    }
}

impl Error for DownloadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Storage(error) | Self::Transfer(error) => Some(error),
            Self::Network(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
