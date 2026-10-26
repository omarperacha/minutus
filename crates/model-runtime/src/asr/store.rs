use std::{
    fmt,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::{
    DownloadCancellation, DownloadError, DownloadProgress, ModelDownloadManager, SpeechProfile,
    VerifiedModelArtifact,
};

/// Content-free installation state, independent of recognizer availability.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpeechModelStatus {
    /// No published artifact; retained partial bytes can be resumed.
    Missing,
    /// Complete digest and format verification passed.
    Installed,
    /// Bytes changed or no longer match the qualified format/runtime.
    Incompatible,
    /// Installation, removal, verification or recognition currently owns the store.
    Busy,
}

/// Serializes speech installation/removal with verified recognizer use.
pub struct SpeechModelStore {
    downloads: ModelDownloadManager,
    operation: Arc<AtomicBool>,
}

/// A verified artifact whose removal is excluded for the lease's lifetime.
/// Keep this lease until the recognizer has released the weights.
pub struct SpeechModelLease {
    artifact: VerifiedModelArtifact,
    operation: Arc<AtomicBool>,
}

impl SpeechModelLease {
    /// Host-only model path and identity for the recognition adapter.
    #[must_use]
    pub const fn artifact(&self) -> &VerifiedModelArtifact {
        &self.artifact
    }
}

struct OperationGuard {
    operation: Arc<AtomicBool>,
    release_on_drop: bool,
}
impl OperationGuard {
    fn into_lease(mut self, artifact: VerifiedModelArtifact) -> SpeechModelLease {
        self.release_on_drop = false;
        SpeechModelLease {
            artifact,
            operation: Arc::clone(&self.operation),
        }
    }
}
impl Drop for OperationGuard {
    fn drop(&mut self) {
        if self.release_on_drop {
            self.operation.store(false, Ordering::Release);
        }
    }
}
impl Drop for SpeechModelLease {
    fn drop(&mut self) {
        self.operation.store(false, Ordering::Release);
    }
}

impl SpeechModelStore {
    /// Prepares private optional speech storage; downloads nothing.
    ///
    /// # Errors
    /// Returns a storage/client error if initialization fails.
    pub fn initialize(root: impl AsRef<Path>) -> Result<Self, DownloadError> {
        Ok(Self {
            downloads: ModelDownloadManager::initialize(root)?,
            operation: Arc::new(AtomicBool::new(false)),
        })
    }

    fn lock(&self) -> Result<OperationGuard, SpeechModelError> {
        self.operation
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| SpeechModelError::Busy)?;
        Ok(OperationGuard {
            operation: Arc::clone(&self.operation),
            release_on_drop: true,
        })
    }

    /// Verifies installed bytes locally, without contacting the network.
    ///
    /// # Errors
    /// Reports storage failures; invalid or missing bytes are explicit statuses.
    pub fn status(&self, profile: &SpeechProfile) -> Result<SpeechModelStatus, SpeechModelError> {
        let _operation = match self.lock() {
            Err(SpeechModelError::Busy) => return Ok(SpeechModelStatus::Busy),
            result => result?,
        };
        if profile.runtime_revision() != super::WHISPER_RUNTIME_REVISION {
            return Ok(SpeechModelStatus::Incompatible);
        }
        match self
            .downloads
            .verify_installed(profile.artifact(), &DownloadCancellation::new())
        {
            Ok(_) => Ok(SpeechModelStatus::Installed),
            Err(DownloadError::Storage(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(SpeechModelStatus::Missing)
            }
            Err(DownloadError::InstalledArtifactInvalid | DownloadError::IncompatibleArtifact) => {
                Ok(SpeechModelStatus::Incompatible)
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Checks whether a regular, correctly sized installed candidate is present.
    ///
    /// This is a fast pre-capture check only. `acquire` independently verifies
    /// the complete digest and format before recognition uses the artifact.
    ///
    /// # Errors
    /// Returns busy during another speech-model operation or reports storage failure.
    pub fn has_install_candidate(&self, profile: &SpeechProfile) -> Result<bool, SpeechModelError> {
        let _operation = self.lock()?;
        if profile.runtime_revision() != super::WHISPER_RUNTIME_REVISION {
            return Ok(false);
        }
        let path = self.downloads.installed_path(profile.artifact());
        match std::fs::symlink_metadata(path) {
            Ok(metadata) => Ok(metadata.file_type().is_file()
                && metadata.len() == profile.artifact().download_bytes()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(DownloadError::Storage(error).into()),
        }
    }

    /// Resumes and publishes only digest-verified, compatible weights.
    ///
    /// # Errors
    /// Rejects active use, incompatible profiles, cancellation and failed downloads.
    pub fn install(
        &self,
        profile: &SpeechProfile,
        cancellation: &DownloadCancellation,
        progress: impl FnMut(DownloadProgress),
    ) -> Result<(), SpeechModelError> {
        let _operation = self.lock()?;
        compatible(profile)?;
        self.downloads
            .download(profile.artifact(), cancellation, progress)?;
        Ok(())
    }

    /// Removes installed and partial bytes, provided no recognizer holds a lease.
    ///
    /// # Errors
    /// Returns busy during active use/installation, or a storage error.
    pub fn remove(&self, profile: &SpeechProfile) -> Result<(), SpeechModelError> {
        let _operation = self.lock()?;
        self.downloads.remove_artifact(profile.artifact())?;
        Ok(())
    }

    /// Rechecks integrity before recognition and excludes concurrent removal.
    ///
    /// # Errors
    /// Rejects missing/changed weights, incompatible runtimes, cancellation and active use.
    pub fn acquire(
        &self,
        profile: &SpeechProfile,
        cancellation: &DownloadCancellation,
    ) -> Result<SpeechModelLease, SpeechModelError> {
        let operation = self.lock()?;
        compatible(profile)?;
        let artifact = self
            .downloads
            .verify_installed(profile.artifact(), cancellation)?;
        Ok(operation.into_lease(artifact))
    }
}

fn compatible(profile: &SpeechProfile) -> Result<(), SpeechModelError> {
    if profile.runtime_revision() != super::WHISPER_RUNTIME_REVISION {
        return Err(DownloadError::IncompatibleArtifact.into());
    }
    Ok(())
}

/// Speech storage failure, with no audio or transcript content.
#[derive(Debug)]
pub enum SpeechModelError {
    /// Another installation or recognizer owns the weights.
    Busy,
    /// Verified download/storage failed.
    Download(DownloadError),
}

impl From<DownloadError> for SpeechModelError {
    fn from(error: DownloadError) -> Self {
        Self::Download(error)
    }
}
impl fmt::Display for SpeechModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => f.write_str(
                "the speech model is in use; finish or cancel the current operation first",
            ),
            Self::Download(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for SpeechModelError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Busy => None,
            Self::Download(error) => Some(error),
        }
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
