use super::{
    AppCore, AppCoreError, Component, ComputationDiagnosticStage, DiagnosticEvent,
    DiagnosticExportPlan, DiagnosticExportReceipt, DiagnosticOutcome, GenerationFailureCategory,
    LogWriteOutcome, Operation, Path, SettingValue,
};

#[cfg(debug_assertions)]
mod development_trace;

#[cfg(debug_assertions)]
pub(crate) use development_trace::DevelopmentToolTrace;

impl AppCore {
    /// Best-effort records one allow-listed operational event.
    ///
    /// The outcome is intentionally separate from application control flow;
    /// callers must continue the security-relevant operation when logging fails.
    #[must_use]
    pub fn record_diagnostic(&self, event: DiagnosticEvent) -> LogWriteOutcome {
        self.diagnostics.record(event)
    }

    /// Best-effort records one content-free sandbox computation outcome.
    ///
    /// Source, arguments, inputs, paths, output and error text are structurally
    /// absent from this allow-listed diagnostic event.
    pub fn record_computation_diagnostic(
        &self,
        status: agent_core::ComputationTerminalStatus,
        duration_ms: u64,
    ) -> LogWriteOutcome {
        let outcome = match status {
            agent_core::ComputationTerminalStatus::Completed => DiagnosticOutcome::Succeeded,
            agent_core::ComputationTerminalStatus::Cancelled => DiagnosticOutcome::Cancelled,
            agent_core::ComputationTerminalStatus::InvalidRequest
            | agent_core::ComputationTerminalStatus::CompileFailed
            | agent_core::ComputationTerminalStatus::RuntimeFailed
            | agent_core::ComputationTerminalStatus::WallLimit
            | agent_core::ComputationTerminalStatus::CpuLimit
            | agent_core::ComputationTerminalStatus::MemoryLimit
            | agent_core::ComputationTerminalStatus::OutputLimit
            | agent_core::ComputationTerminalStatus::PolicyUnavailable
            | agent_core::ComputationTerminalStatus::RunnerFailed => DiagnosticOutcome::Failed,
        };
        self.record_diagnostic(DiagnosticEvent::OperationCompleted {
            component: Component::Computation,
            operation: Operation::ExecuteTool,
            outcome,
            duration_ms: duration_ms.min(60_000),
        })
    }

    /// Best-effort records which content-free post-execution boundary failed.
    ///
    /// The closed component/operation pair identifies the adapter stage without
    /// retaining source, arguments, inputs, paths, output or raw error text.
    pub fn record_computation_stage_failure(
        &self,
        stage: ComputationDiagnosticStage,
        duration_ms: u64,
    ) -> LogWriteOutcome {
        let (component, operation) = stage.diagnostic_coordinates();
        self.record_diagnostic(DiagnosticEvent::OperationCompleted {
            component,
            operation,
            outcome: DiagnosticOutcome::Failed,
            duration_ms: duration_ms.min(60_000),
        })
    }

    /// Records raw local tool-loop state in debug builds only.
    ///
    /// Development traces may contain workspace paths, arguments and results.
    /// They are stored separately and are never part of diagnostic exports.
    #[cfg(debug_assertions)]
    pub fn record_development_tool_trace(&self, event: &serde_json::Value) {
        self.development_tool_trace.record(event);
    }

    /// Best-effort records one content-free chat generation failure category.
    pub fn record_generation_failure(
        &self,
        category: GenerationFailureCategory,
    ) -> LogWriteOutcome {
        self.record_diagnostic(DiagnosticEvent::GenerationFailed { category })
    }

    /// Reports whether local diagnostic logging is enabled.
    #[must_use]
    pub fn diagnostic_logging_enabled(&self) -> bool {
        self.diagnostics.is_enabled()
    }

    /// Persists and applies the user's local diagnostic logging preference.
    ///
    /// # Errors
    ///
    /// Returns an error when the preference cannot be persisted. Logging state
    /// remains unchanged in that case.
    pub fn set_diagnostic_logging(
        &self,
        enabled: bool,
        updated_at: &str,
    ) -> Result<LogWriteOutcome, AppCoreError> {
        self.with_storage(|storage| {
            storage.set_setting(&SettingValue::DiagnosticLogging(enabled), updated_at)
        })?;
        let outcome = if enabled {
            self.diagnostics.set_enabled(true);
            self.record_diagnostic(DiagnosticEvent::LoggingChanged { enabled: true })
        } else {
            let outcome =
                self.record_diagnostic(DiagnosticEvent::LoggingChanged { enabled: false });
            self.diagnostics.set_enabled(false);
            outcome
        };
        Ok(outcome)
    }

    /// Snapshots local diagnostics and returns the mandatory export preview.
    ///
    /// # Errors
    ///
    /// Returns a sanitized error when bounded local logs cannot be read.
    pub fn plan_diagnostic_export(&self) -> Result<DiagnosticExportPlan, AppCoreError> {
        let plan = self.diagnostics.plan_export();
        let outcome = if plan.is_ok() {
            DiagnosticOutcome::Succeeded
        } else {
            DiagnosticOutcome::Failed
        };
        let _ = self.record_diagnostic(DiagnosticEvent::OperationCompleted {
            component: Component::Application,
            operation: Operation::PrepareExport,
            outcome,
            duration_ms: 0,
        });
        plan.map_err(Into::into)
    }

    /// Writes a previously previewed diagnostic snapshot to a new local file.
    ///
    /// # Errors
    ///
    /// Returns a sanitized error when the destination exists or cannot be written.
    pub fn write_diagnostic_export(
        &self,
        plan: DiagnosticExportPlan,
        destination: impl AsRef<Path>,
    ) -> Result<DiagnosticExportReceipt, AppCoreError> {
        let result = plan.write_to(destination);
        let outcome = if result.is_ok() {
            DiagnosticOutcome::Succeeded
        } else {
            DiagnosticOutcome::Failed
        };
        let _ = self.record_diagnostic(DiagnosticEvent::OperationCompleted {
            component: Component::Application,
            operation: Operation::WriteExport,
            outcome,
            duration_ms: 0,
        });
        result.map_err(Into::into)
    }
}
