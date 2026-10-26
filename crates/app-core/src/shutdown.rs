//! Idempotent application shutdown across native and worker-owned references.

use std::sync::atomic::Ordering;

use super::{AppCore, AppCoreError, DiagnosticEvent, DictationCancelReason};

impl AppCore {
    /// Cancels active work and synchronously stops every managed inference process.
    ///
    /// This explicit boundary is required because detached generation workers can
    /// retain an `Arc<AppCore>` after Tauri begins exiting. Waiting for `Drop`
    /// would therefore leave a managed sidecar alive when the OS ends the app.
    /// Repeated exit notifications retry process cleanup but emit the stopped
    /// diagnostic only once.
    ///
    /// # Errors
    ///
    /// Returns the first inference shutdown error after still attempting to stop
    /// both managed runtimes.
    pub fn shutdown(&self) -> Result<(), AppCoreError> {
        let first_request = !self.shutdown_started.swap(true, Ordering::AcqRel);

        self.cancel_active_dictation(DictationCancelReason::WindowClosed);
        if let Ok(generations) = self.ollama_generations.lock() {
            for cancellation in generations.values() {
                cancellation.cancel();
            }
        }

        let ollama_result = self.ollama_shutdown.shutdown();
        let model_runtime_result = self
            .model_runtime
            .unload_model()
            .map_err(AppCoreError::from);
        if first_request {
            let _ = self.record_diagnostic(DiagnosticEvent::ApplicationStopped);
        }

        model_runtime_result?;
        ollama_result
    }
}
