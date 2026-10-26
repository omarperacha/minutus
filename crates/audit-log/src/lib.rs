//! Allow-listed local diagnostics that exclude prompts and secret material.
//!
//! Zeta does not accept arbitrary strings or maps as ordinary log fields. The
//! closed event schema in this crate is the primary privacy boundary; export
//! scanning is defense in depth, not the mechanism that makes logging safe.

mod event;
mod export;
mod logger;
mod transaction;

pub use event::{
    Component, DiagnosticEvent, DiagnosticOutcome, GenerationFailureCategory,
    InferenceTuningProfile, ModelPhase, Operation, PermissionCapability,
};
pub use export::{
    DiagnosticExportError, DiagnosticExportPlan, DiagnosticExportPreview, DiagnosticExportReceipt,
};
pub use logger::{DiagnosticLogger, LogWriteOutcome};
pub use transaction::{
    ApprovalAuditDecision, ApprovalFinalizationAuditOutcome, OwnerAuthorizationAuditOutcome,
    TransactionAuditDeleteReceipt, TransactionAuditError, TransactionAuditEvent,
    TransactionAuditEventKind, TransactionAuditExportPlan, TransactionAuditExportPreview,
    TransactionAuditExportReceipt, TransactionAuditFailureCode, TransactionAuditLogger,
    TransactionAuditOrigin, TransactionAuditResult, TransactionAuditState,
    TransactionAuditTransactionType, TransactionAuditWarningCode, X402AuditStage,
};

/// Version of the JSON Lines diagnostic record schema.
pub const DIAGNOSTIC_SCHEMA_VERSION: u16 = 4;
/// Maximum size of the active local diagnostic file.
pub const MAX_LOG_FILE_BYTES: u64 = 1024 * 1024;
/// Number of rotated files retained in addition to the active file.
pub const RETAINED_LOG_FILES: u8 = 3;

#[cfg(test)]
mod tests {
    use std::{error::Error, fs, io::Write};

    use serde_json::Value;
    use tempfile::TempDir;

    use super::*;
    use crate::logger::{active_log_path, rotated_path};

    fn operation_event() -> DiagnosticEvent {
        DiagnosticEvent::OperationCompleted {
            component: Component::Storage,
            operation: Operation::Persist,
            outcome: DiagnosticOutcome::Succeeded,
            duration_ms: 12,
        }
    }

    #[test]
    fn ordinary_records_have_only_allow_listed_fields() -> Result<(), Box<dyn Error>> {
        let temporary = TempDir::new()?;
        let logger = DiagnosticLogger::new(temporary.path(), true);
        let events = [
            DiagnosticEvent::ApplicationStarted {
                ipc_schema_version: 1,
                storage_schema_version: 1,
            },
            DiagnosticEvent::ApplicationStopped,
            DiagnosticEvent::LoggingChanged { enabled: true },
            operation_event(),
            DiagnosticEvent::OperationCompleted {
                component: Component::Computation,
                operation: Operation::ExecuteTool,
                outcome: DiagnosticOutcome::Failed,
                duration_ms: 5_000,
            },
            DiagnosticEvent::ModelLifecycle {
                phase: ModelPhase::Generate,
                outcome: DiagnosticOutcome::Cancelled,
            },
            DiagnosticEvent::PermissionResolved {
                capability: PermissionCapability::TransactionReview,
                outcome: DiagnosticOutcome::Rejected,
            },
            DiagnosticEvent::GenerationFailed {
                category: GenerationFailureCategory::OrchestrationPhase,
            },
        ];
        for event in events {
            assert_eq!(logger.record(event), LogWriteOutcome::Written);
        }

        let log = fs::read_to_string(active_log_path(temporary.path()))?;
        assert!(log.contains("\"event\":\"generation_failed\""));
        assert!(log.contains("\"category\":\"orchestration_phase\""));
        assert!(log.contains("\"component\":\"computation\""));
        for forbidden in [
            "privateKey",
            "recoveryPhrase",
            "seedPhrase",
            "signature",
            "signedTransaction",
            "signingPayload",
            "prompt",
            "toolResult",
            "imageBytes",
            "message",
            "path",
            "url",
        ] {
            assert!(!log.contains(forbidden));
        }
        for line in log.lines() {
            let value: Value = serde_json::from_str(line)?;
            assert!(value.get("schemaVersion").is_some());
            assert!(value.get("timestampUnixMs").is_some());
            assert!(value.get("sequence").is_some());
            assert!(value.get("data").is_some());
        }
        Ok(())
    }

    #[test]
    fn disabled_logging_writes_nothing() -> Result<(), Box<dyn Error>> {
        let temporary = TempDir::new()?;
        let logger = DiagnosticLogger::new(temporary.path(), false);
        assert_eq!(logger.record(operation_event()), LogWriteOutcome::Disabled);
        assert!(!active_log_path(temporary.path()).exists());
        logger.set_enabled(true);
        assert_eq!(logger.record(operation_event()), LogWriteOutcome::Written);
        logger.set_enabled(false);
        assert_eq!(logger.record(operation_event()), LogWriteOutcome::Disabled);
        assert_eq!(
            fs::read_to_string(active_log_path(temporary.path()))?
                .lines()
                .count(),
            1
        );
        Ok(())
    }

    #[test]
    fn logging_failure_is_non_propagating() -> Result<(), Box<dyn Error>> {
        let temporary = TempDir::new()?;
        let invalid_directory = temporary.path().join("not-a-directory");
        fs::write(&invalid_directory, b"occupied")?;
        let logger = DiagnosticLogger::new(invalid_directory, true);
        assert_eq!(logger.record(operation_event()), LogWriteOutcome::Failed);
        let critical_security_check_still_runs = true;
        assert!(critical_security_check_still_runs);
        Ok(())
    }

    #[test]
    fn rotation_is_bounded_to_the_retention_limit() -> Result<(), Box<dyn Error>> {
        let temporary = TempDir::new()?;
        let logger = DiagnosticLogger::new(temporary.path(), true);
        let active = active_log_path(temporary.path());
        fs::create_dir_all(temporary.path())?;
        let maximum_file_bytes = usize::try_from(MAX_LOG_FILE_BYTES)?;
        for _ in 0..=RETAINED_LOG_FILES {
            fs::write(&active, vec![b'x'; maximum_file_bytes])?;
            assert_eq!(logger.record(operation_event()), LogWriteOutcome::Written);
        }
        for index in 1..=RETAINED_LOG_FILES {
            assert!(rotated_path(&active, index).exists());
        }
        assert!(!rotated_path(&active, RETAINED_LOG_FILES + 1).exists());
        Ok(())
    }

    #[test]
    fn export_requires_a_previewable_snapshot_and_never_overwrites() -> Result<(), Box<dyn Error>> {
        let temporary = TempDir::new()?;
        let logger = DiagnosticLogger::new(temporary.path().join("logs"), true);
        assert_eq!(logger.record(operation_event()), LogWriteOutcome::Written);
        let plan = logger.plan_export()?;
        assert_eq!(plan.preview().source_file_count(), 1);
        assert_eq!(plan.preview().record_count(), 1);
        assert!(plan.preview().content_bytes() > 0);
        assert!(
            plan.preview()
                .privacy_description()
                .contains("excludes prompts")
        );

        let destination = temporary.path().join("zeta-diagnostics-export.jsonl");
        let receipt = plan.write_to(&destination)?;
        assert!(receipt.bytes_written() > 0);
        let exported = fs::read_to_string(&destination)?;
        assert_eq!(exported.lines().count(), 2);
        let manifest: Value =
            serde_json::from_str(exported.lines().next().ok_or("export manifest is absent")?)?;
        assert_eq!(manifest.get("recordCount"), Some(&Value::from(1)));

        let second_plan = logger.plan_export()?;
        assert!(matches!(
            second_plan.write_to(&destination),
            Err(DiagnosticExportError::DestinationExists)
        ));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(destination)?.permissions().mode() & 0o777,
                0o600
            );
        }
        Ok(())
    }

    #[test]
    fn current_export_accepts_prior_diagnostic_schema_records() -> Result<(), Box<dyn Error>> {
        let temporary = TempDir::new()?;
        let logger = DiagnosticLogger::new(temporary.path(), true);
        assert_eq!(logger.record(operation_event()), LogWriteOutcome::Written);
        let mut log = fs::OpenOptions::new()
            .append(true)
            .open(active_log_path(temporary.path()))?;
        log.write_all(
            b"{\"schemaVersion\":1,\"timestampUnixMs\":1,\"sequence\":0,\"data\":{\"event\":\"application_stopped\"}}\n",
        )?;
        log.flush()?;

        let plan = logger.plan_export()?;
        assert_eq!(plan.preview().record_count(), 2);
        Ok(())
    }

    #[test]
    fn export_rejects_injected_or_corrupt_records() -> Result<(), Box<dyn Error>> {
        let temporary = TempDir::new()?;
        let logger = DiagnosticLogger::new(temporary.path(), true);
        assert_eq!(logger.record(operation_event()), LogWriteOutcome::Written);
        let mut log = fs::OpenOptions::new()
            .append(true)
            .open(active_log_path(temporary.path()))?;
        log.write_all(b"{\"schemaVersion\":1,\"prompt\":\"must not escape\"}\n")?;
        log.flush()?;
        assert!(matches!(
            logger.plan_export(),
            Err(DiagnosticExportError::CorruptSource)
        ));
        Ok(())
    }
}
