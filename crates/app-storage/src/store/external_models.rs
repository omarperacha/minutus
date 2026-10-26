//! Persisted references to user-owned files; this repository never touches their bytes.
use serde::{Deserialize, Serialize};

use super::{ApplicationStore, OptionalExtension, StorageError, params};

/// Verified external artifact location. Host paths never cross model IPC.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalModelFile {
    /// Catalogue-local artifact identity.
    pub artifact_id: String,
    /// Absolute path chosen through native UI.
    pub path: String,
    /// Verified content identity.
    pub sha256: String,
    /// Verified length.
    pub bytes: u64,
    /// Change-detection hint, never a substitute for hashing before load.
    pub modified_nanos: String,
    /// Native-only persistent read authority. Absent in older registrations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_bookmark: Option<Vec<u8>>,
}

impl std::fmt::Debug for ExternalModelFile {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExternalModelFile")
            .field("artifact_id", &self.artifact_id)
            .field("path", &self.path)
            .field("sha256", &self.sha256)
            .field("bytes", &self.bytes)
            .field("modified_nanos", &self.modified_nanos)
            .field("has_access_bookmark", &self.access_bookmark.is_some())
            .finish()
    }
}

/// A successful registration persists even when its files become unavailable.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalModelRegistration {
    /// Exact catalogue model key.
    pub model_id: String,
    /// Catalogue identity whose managed receipt is superseded atomically.
    pub selection: ipc_types::ModelSelection,
    /// A failed content check remains visible until successful re-registration.
    pub verification_failed: bool,
    /// Independently verified modules, possibly incomplete, in catalogue order.
    pub files: Vec<ExternalModelFile>,
}

impl ExternalModelRegistration {
    fn validate(&self) -> Result<(), StorageError> {
        let invalid = || StorageError::InvalidRecord("invalid external model registration");
        if self.selection.model_id.is_empty()
            || self.selection.model_id.len() > 256
            || self.selection.model_version.is_empty()
            || self.selection.model_version.len() > 256
            || self.model_id.is_empty()
            || self.model_id.len() > 1024
            || self.files.is_empty()
            || self.files.len() > 16
        {
            return Err(invalid());
        }
        let mut ids = std::collections::HashSet::new();
        for file in &self.files {
            if file.artifact_id.is_empty()
                || file
                    .access_bookmark
                    .as_ref()
                    .is_some_and(|data| data.is_empty() || data.len() > 65_536)
                || !ids.insert(&file.artifact_id)
                || !std::path::Path::new(&file.path).is_absolute()
                || file.path.len() > 16_384
                || file.path.contains('\0')
                || file.sha256.len() != 64
                || !file
                    .sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                || file.bytes == 0
                || file.modified_nanos.parse::<u128>().is_err()
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
}

impl ApplicationStore {
    /// Atomically replaces locations only after the caller verifies the entire bundle.
    ///
    /// # Errors
    /// Rejects malformed records and database failures.
    pub fn save_external_model(
        &mut self,
        registration: &ExternalModelRegistration,
    ) -> Result<(), StorageError> {
        registration.validate()?;
        let record = serde_json::to_string(registration)
            .map_err(|_| StorageError::InvalidRecord("invalid external model registration"))?;
        let transaction = self
            .connection
            .transaction_with_behavior(super::TransactionBehavior::Immediate)?;
        transaction.execute("DELETE FROM models WHERE id = ?1", [&registration.model_id])?;
        transaction.execute(
            "DELETE FROM provider_model_installs WHERE model_id = ?1 AND model_version = ?2",
            params![
                registration.selection.model_id,
                registration.selection.model_version
            ],
        )?;
        transaction.execute("INSERT INTO external_model_registrations (model_id, record) VALUES (?1, ?2) ON CONFLICT(model_id) DO UPDATE SET record = excluded.record", params![registration.model_id, record])?;
        transaction.commit()?;
        Ok(())
    }

    /// Reads a registration without dropping it when files disappear.
    ///
    /// # Errors
    /// Rejects corrupt persisted records and database failures.
    pub fn external_model(
        &self,
        model_id: &str,
    ) -> Result<Option<ExternalModelRegistration>, StorageError> {
        let record: Option<String> = self
            .connection
            .query_row(
                "SELECT record FROM external_model_registrations WHERE model_id = ?1",
                [model_id],
                |row| row.get(0),
            )
            .optional()?;
        record
            .map(|record| {
                let registration: ExternalModelRegistration = serde_json::from_str(&record)
                    .map_err(|_| {
                        StorageError::InvalidRecord("invalid external model registration")
                    })?;
                registration.validate()?;
                if registration.model_id != model_id {
                    return Err(StorageError::InvalidRecord("external model owner mismatch"));
                }
                Ok(registration)
            })
            .transpose()
    }

    /// Forgets metadata only; never opens, removes or modifies external files.
    ///
    /// # Errors
    /// Returns database failures.
    pub fn forget_external_model(&self, model_id: &str) -> Result<(), StorageError> {
        self.connection.execute(
            "DELETE FROM external_model_registrations WHERE model_id = ?1",
            [model_id],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_survives_reopen_and_forget_leaves_external_bytes()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let weights = directory.path().join("weights.gguf");
        std::fs::write(&weights, b"external weights")?;
        let db = directory.path().join("app.sqlite");
        let registration = ExternalModelRegistration {
            model_id: "model@revision".into(),
            selection: ipc_types::ModelSelection {
                model_id: "model".into(),
                model_version: "revision".into(),
            },
            verification_failed: false,
            files: vec![ExternalModelFile {
                artifact_id: "primary".into(),
                path: weights.to_string_lossy().into_owned(),
                sha256: "a".repeat(64),
                bytes: 16,
                modified_nanos: "1".into(),
                access_bookmark: Some(b"opaque native bookmark".to_vec()),
            }],
        };
        let mut store = ApplicationStore::open(&db)?;
        let debug = format!("{registration:?}");
        assert!(debug.contains("has_access_bookmark: true"));
        assert!(!debug.contains(&format!("{:?}", b"opaque native bookmark")));
        assert!(store.external_model(&registration.model_id)?.is_none());
        store.save_external_model(&registration)?;
        drop(store);
        let store = ApplicationStore::open(&db)?;
        assert_eq!(
            store.external_model(&registration.model_id)?,
            Some(registration.clone())
        );
        store.forget_external_model(&registration.model_id)?;
        assert!(store.external_model(&registration.model_id)?.is_none());
        assert_eq!(std::fs::read(weights)?, b"external weights");
        Ok(())
    }
}

#[cfg(test)]
mod replacement_tests {
    use super::*;
    #[test]
    fn failed_publication_preserves_previous_managed_receipt()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let mut store = ApplicationStore::open(directory.path().join("db.sqlite"))?;
        let path = directory.path().join("weights.gguf");
        std::fs::write(&path, b"weights")?;
        let managed = crate::ModelMetadata {
            id: "artifact".into(),
            display_name: "Model".into(),
            format: "gguf".into(),
            local_path: path.to_string_lossy().into_owned(),
            content_digest: "a".repeat(64),
            byte_size: 7,
            imported_at: "2026-10-20T00:00:00Z".into(),
        };
        store.save_model(&managed)?;
        let registration = ExternalModelRegistration {
            model_id: managed.id.clone(),
            selection: ipc_types::ModelSelection {
                model_id: "model".into(),
                model_version: "revision".into(),
            },
            verification_failed: false,
            files: vec![ExternalModelFile {
                artifact_id: "primary".into(),
                path: managed.local_path.clone(),
                sha256: managed.content_digest.clone(),
                bytes: 7,
                modified_nanos: "1".into(),
                access_bookmark: None,
            }],
        };
        store.connection.execute_batch("CREATE TRIGGER reject_external BEFORE INSERT ON external_model_registrations BEGIN SELECT RAISE(ABORT, 'test publication failure'); END;")?;
        assert!(store.save_external_model(&registration).is_err());
        assert_eq!(store.model(&managed.id)?, Some(managed.clone()));
        assert!(store.external_model(&managed.id)?.is_none());
        store
            .connection
            .execute_batch("DROP TRIGGER reject_external;")?;
        store.save_external_model(&registration)?;
        assert!(store.model(&managed.id)?.is_none());
        assert_eq!(store.external_model(&managed.id)?, Some(registration));
        assert_eq!(std::fs::read(path)?, b"weights");
        Ok(())
    }
}
