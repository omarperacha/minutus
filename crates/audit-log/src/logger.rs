//! Bounded local JSON Lines writer.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    DIAGNOSTIC_SCHEMA_VERSION, DiagnosticEvent, DiagnosticExportError, DiagnosticExportPlan,
    MAX_LOG_FILE_BYTES, RETAINED_LOG_FILES, event::DiagnosticRecord, export::build_export_plan,
};

const ACTIVE_LOG_NAME: &str = "zeta-diagnostics.jsonl";

/// Non-sensitive outcome of a best-effort local log write.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogWriteOutcome {
    /// The event was appended locally.
    Written,
    /// Local logging is disabled.
    Disabled,
    /// The event could not be recorded; application work must continue.
    Failed,
}

/// Thread-safe, bounded local diagnostic logger.
pub struct DiagnosticLogger {
    directory: PathBuf,
    enabled: AtomicBool,
    sequence: AtomicU64,
    write_lock: Mutex<()>,
}

impl DiagnosticLogger {
    /// Configures diagnostics beneath a Rust-owned application-data directory.
    #[must_use]
    pub fn new(directory: impl Into<PathBuf>, enabled: bool) -> Self {
        Self {
            directory: directory.into(),
            enabled: AtomicBool::new(enabled),
            sequence: AtomicU64::new(0),
            write_lock: Mutex::new(()),
        }
    }

    /// Reports whether local diagnostics are currently enabled.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    /// Enables or disables future local writes without performing I/O.
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Release);
    }

    /// Best-effort appends one allow-listed event.
    ///
    /// This method deliberately does not return an error. Logging failure must
    /// never bypass validation, authorization, signing or another application
    /// security decision.
    #[must_use]
    pub fn record(&self, event: DiagnosticEvent) -> LogWriteOutcome {
        if !self.is_enabled() {
            return LogWriteOutcome::Disabled;
        }
        let Ok(_guard) = self.write_lock.lock() else {
            return LogWriteOutcome::Failed;
        };
        match self.write_record(event) {
            Ok(()) => LogWriteOutcome::Written,
            Err(()) => LogWriteOutcome::Failed,
        }
    }

    /// Snapshots existing local logs into a plan that must be previewed before export.
    ///
    /// # Errors
    ///
    /// Returns a sanitized error when log files cannot be read or exceed the
    /// bounded export size.
    pub fn plan_export(&self) -> Result<DiagnosticExportPlan, DiagnosticExportError> {
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| DiagnosticExportError::Unavailable)?;
        build_export_plan(&self.directory, ACTIVE_LOG_NAME, RETAINED_LOG_FILES)
    }

    fn write_record(&self, event: DiagnosticEvent) -> Result<(), ()> {
        prepare_private_directory(&self.directory).map_err(|_| ())?;
        let record = DiagnosticRecord {
            schema_version: DIAGNOSTIC_SCHEMA_VERSION,
            timestamp_unix_ms: timestamp_unix_ms(),
            sequence: self.sequence.fetch_add(1, Ordering::Relaxed),
            data: event,
        };
        let mut encoded = serde_json::to_vec(&record).map_err(|_| ())?;
        encoded.push(b'\n');
        let active_path = self.directory.join(ACTIVE_LOG_NAME);
        rotate_if_needed(&active_path, encoded.len() as u64).map_err(|_| ())?;
        let mut file = private_append_file(&active_path).map_err(|_| ())?;
        file.write_all(&encoded).map_err(|_| ())?;
        file.flush().map_err(|_| ())
    }
}

pub(crate) fn timestamp_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

pub(crate) fn rotate_if_needed(
    active_path: &Path,
    incoming_bytes: u64,
) -> Result<(), std::io::Error> {
    let current_bytes = match fs::metadata(active_path) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error),
    };
    if current_bytes.saturating_add(incoming_bytes) <= MAX_LOG_FILE_BYTES {
        return Ok(());
    }

    let oldest = rotated_path(active_path, RETAINED_LOG_FILES);
    remove_if_present(&oldest)?;
    for index in (1..RETAINED_LOG_FILES).rev() {
        let source = rotated_path(active_path, index);
        let destination = rotated_path(active_path, index + 1);
        rename_if_present(&source, &destination)?;
    }
    rename_if_present(active_path, &rotated_path(active_path, 1))
}

pub(crate) fn rotated_path(active_path: &Path, index: u8) -> PathBuf {
    let mut name = active_path.as_os_str().to_os_string();
    name.push(format!(".{index}"));
    PathBuf::from(name)
}

pub(crate) fn remove_if_present(path: &Path) -> Result<(), std::io::Error> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn rename_if_present(source: &Path, destination: &Path) -> Result<(), std::io::Error> {
    match fs::rename(source, destination) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub(crate) fn prepare_private_directory(path: &Path) -> Result<(), std::io::Error> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub(crate) fn private_create_new(path: &Path) -> Result<fs::File, std::io::Error> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

pub(crate) fn private_append_file(path: &Path) -> Result<fs::File, std::io::Error> {
    let mut options = fs::OpenOptions::new();
    options.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}

#[cfg(test)]
pub(crate) fn active_log_path(directory: &Path) -> PathBuf {
    directory.join(ACTIVE_LOG_NAME)
}
