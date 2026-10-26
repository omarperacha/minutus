//! Read-only verification of user-owned catalogue artifacts. No file mutation APIs.
use std::{
    error::Error,
    fmt,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use sha2::{Digest, Sha256};

use crate::ModelArtifact;

/// A read-only verification result, suitable for persistence by the application owner.
pub struct ExternalFileVerification {
    /// Absolute source location, never a managed copy.
    pub path: PathBuf,
    /// File modification hint used only for cheap availability checks.
    pub modified_nanos: String,
}

/// External-file failure with no host paths in its presentation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExternalModelError {
    /// Missing or inaccessible file.
    Unavailable,
    /// Selected bytes do not match this catalogue artifact.
    Mismatch(String),
    /// Files changed during verification or since registration.
    Changed,
    /// The runtime cannot reference external files on this platform/configuration.
    Unsupported,
    /// An explicit external registration must be removed before managed download.
    Registered,
}

impl fmt::Display for ExternalModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => f.write_str("Model unavailable: a file is missing or inaccessible. Use Locate files to restore access."),
            Self::Mismatch(expected) => write!(f, "This file does not match {expected}. Choose the exact supported model revision and quantisation; companion files must also match."),
            Self::Changed => f.write_str("Model verification failed: a registered file changed. Use Locate files to verify replacement files."),
            Self::Registered => f.write_str("Remove the external file registration before downloading this model. Your existing files will remain untouched."),
            Self::Unsupported => f.write_str("This runtime cannot use external files in this configuration."),
        }
    }
}
impl Error for ExternalModelError {}

/// Current read-only file attributes for change detection, not content authentication.
///
/// # Errors
/// Rejects symlinks, nonregular files, inaccessible paths and unsupported timestamps.
pub fn external_file_stamp(path: &Path) -> Result<(u64, String), ExternalModelError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| ExternalModelError::Unavailable)?;
    if !metadata.file_type().is_file() {
        return Err(ExternalModelError::Unavailable);
    }
    // An existing but unreadable file must not be advertised as available.
    File::open(path).map_err(|_| ExternalModelError::Unavailable)?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .ok_or(ExternalModelError::Unavailable)?;
    Ok((metadata.len(), modified.as_nanos().to_string()))
}

/// Hashes an exact catalogue artifact without writing to it or creating a copy.
///
/// # Errors
/// Rejects mismatches, changed sources and inaccessible/nonregular paths.
pub fn verify_external_file(
    artifact: &ModelArtifact,
    path: &Path,
    mut progress: impl FnMut(u64),
) -> Result<ExternalFileVerification, ExternalModelError> {
    let before = external_file_stamp(path)?;
    if before.0 != artifact.installed_bytes() {
        return Err(ExternalModelError::Mismatch(
            artifact.file_name().to_owned(),
        ));
    }
    let mut file = File::open(path).map_err(|_| ExternalModelError::Unavailable)?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0; 1024 * 1024];
    let mut read_bytes = 0_u64;
    progress(0);
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| ExternalModelError::Unavailable)?;
        if count == 0 {
            break;
        }
        read_bytes = read_bytes.saturating_add(count as u64);
        if read_bytes > before.0 {
            return Err(ExternalModelError::Changed);
        }
        digest.update(&buffer[..count]);
        progress(read_bytes);
    }
    if read_bytes != before.0 || external_file_stamp(path)? != before {
        return Err(ExternalModelError::Changed);
    }
    if format!("{:x}", digest.finalize()) != artifact.sha256() {
        return Err(ExternalModelError::Mismatch(
            artifact.file_name().to_owned(),
        ));
    }
    Ok(ExternalFileVerification {
        path: path.to_owned(),
        modified_nanos: before.1,
    })
}
