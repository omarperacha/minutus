//! Persistent read-only model-file scopes owned by the native desktop host.
use app_core::ExternalModelAccess;
use model_runtime::ExternalModelError;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Mutex,
};

use crate::workspace_bookmark::{self, SecurityScopedWorkspace};

/// Scopes live while registered files may be used by inherited runtime children.
/// Replacement/removal stops those children before releasing the corresponding
/// scope; application teardown releases all remaining scopes through RAII.
#[derive(Default)]
pub(crate) struct ModelFileAccess {
    active: Mutex<HashMap<(PathBuf, Vec<u8>), SecurityScopedWorkspace>>,
}

impl ExternalModelAccess for ModelFileAccess {
    fn restore(&self, path: &Path, bookmark: Option<&[u8]>) -> Result<(), ExternalModelError> {
        let Some(bookmark) = bookmark else {
            // Old path-only imports gain no authority here. Ordinary filesystem
            // checks still fail closed until the user explicitly locates them.
            return Ok(());
        };
        if bookmark.is_empty() || bookmark.len() > 65_536 {
            return Err(ExternalModelError::Unavailable);
        }
        let mut active = self
            .active
            .lock()
            .map_err(|_| ExternalModelError::Unavailable)?;
        let key = (path.to_path_buf(), bookmark.to_vec());
        if active.contains_key(&key) {
            return Ok(());
        }
        if active.len() >= 256 {
            return Err(ExternalModelError::Unavailable);
        }
        let scope =
            workspace_bookmark::access(bookmark).map_err(|_| ExternalModelError::Unavailable)?;
        // A bookmark resolving somewhere else must not silently retarget a
        // verified registration. Locate + SHA verification is the recovery path.
        let resolved = scope
            .path()
            .canonicalize()
            .map_err(|_| ExternalModelError::Unavailable)?;
        if resolved != path {
            return Err(ExternalModelError::Unavailable);
        }
        active.insert(key, scope);
        Ok(())
    }

    fn release(&self, path: &Path, bookmark: Option<&[u8]>) {
        if let Some(bookmark) = bookmark
            && let Ok(mut active) = self.active.lock()
        {
            active.remove(&(path.to_path_buf(), bookmark.to_vec()));
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn bookmark_for_import(path: &Path) -> Result<Option<Vec<u8>>, ipc_types::IpcError> {
    workspace_bookmark::create_read_only(path)
        .map(Some)
        .map_err(|()| {
            ipc_types::IpcError::invalid_request(
                "macOS could not retain read access to this model file. Select the file again.",
            )
        })
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn bookmark_for_import(_path: &Path) -> Result<Option<Vec<u8>>, ipc_types::IpcError> {
    Ok(None)
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn stored_file_bookmark_restores_in_a_new_owner_and_releases_exact_scope()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("synthetic.gguf");
        std::fs::write(&path, b"synthetic model fixture")?;
        let path = path.canonicalize()?;
        let bookmark = bookmark_for_import(&path)
            .map_err(|error| format!("{error:?}"))?
            .ok_or("bookmark missing")?;
        let persisted = directory.path().join("bookmark.json");
        std::fs::write(&persisted, serde_json::to_vec(&bookmark)?)?;
        {
            let first = ModelFileAccess::default();
            first.restore(&path, Some(&bookmark))?;
        }
        let reopened: Vec<u8> = serde_json::from_slice(&std::fs::read(&persisted)?)?;
        let second = ModelFileAccess::default();
        second.restore(&path, Some(&reopened))?;
        second.restore(&path, Some(&reopened))?;
        assert_eq!(second.active.lock().map_err(|_| "poisoned")?.len(), 1);
        second.release(&path, Some(b"different bookmark"));
        assert_eq!(second.active.lock().map_err(|_| "poisoned")?.len(), 1);
        second.release(&path, Some(&reopened));
        assert!(second.active.lock().map_err(|_| "poisoned")?.is_empty());
        assert_eq!(std::fs::read(&path)?, b"synthetic model fixture");
        Ok(())
    }

    #[test]
    fn corrupt_or_retargeted_bookmark_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("synthetic.gguf");
        std::fs::write(&path, b"synthetic")?;
        let path = path.canonicalize()?;
        let bookmark = bookmark_for_import(&path)
            .map_err(|error| format!("{error:?}"))?
            .ok_or("bookmark missing")?;
        let access = ModelFileAccess::default();
        assert!(access.restore(&path, Some(b"invalid")).is_err());
        assert!(access.restore(&path, Some(&vec![0; 65_537])).is_err());
        assert!(
            access
                .restore(&path.with_file_name("other.gguf"), Some(&bookmark))
                .is_err()
        );
        assert!(access.active.lock().map_err(|_| "poisoned")?.is_empty());
        Ok(())
    }
}
