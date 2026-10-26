//! Host-owned restoration of platform access, separate from content verification.
use crate::{AppCore, AppCoreError};
use app_storage::ExternalModelFile;
use model_runtime::ExternalModelError;
use std::{path::Path, sync::Arc};

/// Native composition restores only previously user-selected file authority.
/// Paths and opaque bookmark bytes never cross model or frontend IPC.
pub trait ExternalModelAccess: Send + Sync {
    /// Retains read access while this registration can be used by the runtime.
    ///
    /// # Errors
    /// Returns unavailable when the stored authority cannot be restored exactly.
    fn restore(&self, path: &Path, bookmark: Option<&[u8]>) -> Result<(), ExternalModelError>;
    /// Releases access after the runtime is stopped and its reference removed.
    fn release(&self, path: &Path, bookmark: Option<&[u8]>);
}

pub(crate) struct DirectFileAccess;
impl ExternalModelAccess for DirectFileAccess {
    fn restore(&self, _path: &Path, _bookmark: Option<&[u8]>) -> Result<(), ExternalModelError> {
        Ok(())
    }
    fn release(&self, _path: &Path, _bookmark: Option<&[u8]>) {}
}

pub(super) fn restore_file(
    access: &dyn ExternalModelAccess,
    file: &ExternalModelFile,
) -> Result<(), ExternalModelError> {
    access.restore(Path::new(&file.path), file.access_bookmark.as_deref())
}

impl AppCore {
    /// Installs native file-access restoration before the application is shared.
    ///
    /// Existing provider children are stopped first. Registered grants are restored
    /// before the next provider launch so children inherit the selected-file access.
    /// Invalid/inaccessible registrations remain unavailable rather than blocking
    /// unrelated application features during startup.
    ///
    /// # Errors
    /// Returns an error if the previous inference providers cannot be stopped.
    pub fn with_external_model_access(
        mut self,
        access: Arc<dyn ExternalModelAccess>,
    ) -> Result<Self, AppCoreError> {
        self.model_runtime.unload_model()?;
        self.ollama_shutdown.stop_for_model_change()?;
        self.external_model_access = access;
        for model in self.model_catalogue.models() {
            if let Ok(Some(registration)) = self.external_registration(model) {
                let _ = super::availability::external_state(
                    model,
                    &registration,
                    self.external_model_access.as_ref(),
                );
            }
        }
        Ok(self)
    }
}
