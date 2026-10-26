//! Explicit snapshot and export of bounded local diagnostics.

use std::{
    error::Error,
    fmt, fs,
    io::Write,
    path::{Path, PathBuf},
};

use serde::Serialize;

use crate::{
    DIAGNOSTIC_SCHEMA_VERSION, MAX_LOG_FILE_BYTES,
    event::DiagnosticRecord,
    logger::{private_create_new, rotated_path},
};

const EXPORT_SCHEMA_VERSION: u16 = 1;
const PRIVACY_DESCRIPTION: &str = "Local Minutus operational events in JSON Lines format. The event schema excludes prompts, tool-result contents, image bytes, paths, URLs, wallet secrets, signatures, signed transactions and signing payloads.";

/// Sanitized failure while planning or writing a local diagnostic export.
#[derive(Debug)]
pub enum DiagnosticExportError {
    /// Local logs could not be read or the export destination could not be written.
    Io(std::io::Error),
    /// Logger state is temporarily unavailable.
    Unavailable,
    /// Local files exceeded the bounded export budget.
    SizeLimitExceeded,
    /// A local diagnostic file did not match the closed record schema.
    CorruptSource,
    /// The requested destination already exists and will not be overwritten.
    DestinationExists,
}

impl fmt::Display for DiagnosticExportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(_) => formatter.write_str("diagnostic export could not be completed"),
            Self::Unavailable => {
                formatter.write_str("diagnostic export is temporarily unavailable")
            }
            Self::SizeLimitExceeded => {
                formatter.write_str("diagnostic export exceeds its size limit")
            }
            Self::CorruptSource => formatter.write_str("local diagnostics are malformed"),
            Self::DestinationExists => {
                formatter.write_str("diagnostic export destination already exists")
            }
        }
    }
}

impl Error for DiagnosticExportError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Unavailable
            | Self::SizeLimitExceeded
            | Self::CorruptSource
            | Self::DestinationExists => None,
        }
    }
}

impl From<std::io::Error> for DiagnosticExportError {
    fn from(error: std::io::Error) -> Self {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            Self::DestinationExists
        } else {
            Self::Io(error)
        }
    }
}

/// User-reviewable summary of the exact in-memory export snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticExportPreview {
    source_file_count: usize,
    record_count: usize,
    content_bytes: u64,
}

impl DiagnosticExportPreview {
    /// Number of local log files represented by the snapshot.
    #[must_use]
    pub const fn source_file_count(&self) -> usize {
        self.source_file_count
    }

    /// Number of JSON Lines diagnostic records represented by the snapshot.
    #[must_use]
    pub const fn record_count(&self) -> usize {
        self.record_count
    }

    /// Total bytes of diagnostic records before the export manifest.
    #[must_use]
    pub const fn content_bytes(&self) -> u64 {
        self.content_bytes
    }

    /// Static description suitable for display before creating the export.
    #[must_use]
    pub const fn privacy_description(&self) -> &'static str {
        PRIVACY_DESCRIPTION
    }
}

/// Exact bounded snapshot prepared for an explicit local export.
pub struct DiagnosticExportPlan {
    preview: DiagnosticExportPreview,
    files: Vec<Vec<u8>>,
}

impl DiagnosticExportPlan {
    /// Returns the summary that must be presented before export creation.
    #[must_use]
    pub const fn preview(&self) -> &DiagnosticExportPreview {
        &self.preview
    }

    /// Writes this exact snapshot to a new local file without overwriting data.
    ///
    /// # Errors
    ///
    /// Returns a sanitized error when the destination exists or cannot be
    /// created and fully written. A failed write may leave a partial new file.
    pub fn write_to(
        self,
        destination: impl AsRef<Path>,
    ) -> Result<DiagnosticExportReceipt, DiagnosticExportError> {
        let mut destination = private_create_new(destination.as_ref())?;
        let manifest = ExportManifest {
            export_schema_version: EXPORT_SCHEMA_VERSION,
            diagnostic_schema_version: DIAGNOSTIC_SCHEMA_VERSION,
            source_file_count: self.preview.source_file_count,
            record_count: self.preview.record_count,
            content_bytes: self.preview.content_bytes,
            privacy: PRIVACY_DESCRIPTION,
        };
        let mut bytes_written = write_json_line(&mut destination, &manifest)?;
        for bytes in self.files {
            destination.write_all(&bytes)?;
            bytes_written = bytes_written.saturating_add(bytes.len() as u64);
        }
        destination.flush()?;
        Ok(DiagnosticExportReceipt { bytes_written })
    }
}

/// Result of a successful explicit local export.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticExportReceipt {
    bytes_written: u64,
}

impl DiagnosticExportReceipt {
    /// Total bytes written, including the export manifest.
    #[must_use]
    pub const fn bytes_written(self) -> u64 {
        self.bytes_written
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportManifest {
    export_schema_version: u16,
    diagnostic_schema_version: u16,
    source_file_count: usize,
    record_count: usize,
    content_bytes: u64,
    privacy: &'static str,
}

pub(crate) fn build_export_plan(
    directory: &Path,
    active_name: &str,
    retained_files: u8,
) -> Result<DiagnosticExportPlan, DiagnosticExportError> {
    let active_path = directory.join(active_name);
    let mut paths = Vec::<PathBuf>::new();
    for index in (1..=retained_files).rev() {
        paths.push(rotated_path(&active_path, index));
    }
    paths.push(active_path);

    let maximum_export_bytes = MAX_LOG_FILE_BYTES.saturating_mul(u64::from(retained_files) + 1);
    let mut files = Vec::new();
    let mut content_bytes = 0_u64;
    let mut record_count = 0_usize;
    for path in paths {
        let source_bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let bytes = validate_and_canonicalize(&source_bytes)?;
        content_bytes = content_bytes.saturating_add(bytes.len() as u64);
        if content_bytes > maximum_export_bytes {
            return Err(DiagnosticExportError::SizeLimitExceeded);
        }
        let records_in_file = bytes.split(|byte| *byte == b'\n').count().saturating_sub(1);
        record_count = record_count.saturating_add(records_in_file);
        files.push(bytes);
    }
    Ok(DiagnosticExportPlan {
        preview: DiagnosticExportPreview {
            source_file_count: files.len(),
            record_count,
            content_bytes,
        },
        files,
    })
}

fn validate_and_canonicalize(source: &[u8]) -> Result<Vec<u8>, DiagnosticExportError> {
    let mut canonical = Vec::new();
    for line in source.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        let record: DiagnosticRecord =
            serde_json::from_slice(line).map_err(|_| DiagnosticExportError::CorruptSource)?;
        serde_json::to_writer(&mut canonical, &record)
            .map_err(|_| DiagnosticExportError::Unavailable)?;
        canonical.push(b'\n');
    }
    Ok(canonical)
}

fn write_json_line(
    destination: &mut fs::File,
    value: &impl Serialize,
) -> Result<u64, DiagnosticExportError> {
    let mut encoded = serde_json::to_vec(value).map_err(|_| DiagnosticExportError::Unavailable)?;
    encoded.push(b'\n');
    destination.write_all(&encoded)?;
    Ok(encoded.len() as u64)
}
