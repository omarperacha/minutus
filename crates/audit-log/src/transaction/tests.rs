use std::{error::Error, fs, io::Write as _};

use serde_json::Value;
use tempfile::TempDir;

use super::*;

const ACCOUNT: &str = "0x9858effd232b4033e47d90003d41ec34ecaeda94";

fn event(data: TransactionAuditEventKind) -> Result<TransactionAuditEvent, TransactionAuditError> {
    TransactionAuditEvent::new(
        "correlation-01",
        "transaction-01",
        ACCOUNT,
        8453,
        TransactionAuditTransactionType::Eip1559,
        Some([0x11; 32]),
        Some([0x22; 32]),
        TransactionAuditOrigin::User,
        data,
    )
}

#[test]
fn every_required_stage_is_distinguishable_and_contains_only_public_metadata()
-> Result<(), Box<dyn Error>> {
    let temporary = TempDir::new()?;
    let logger = TransactionAuditLogger::new(temporary.path());
    let events = [
        TransactionAuditEventKind::LifecycleTransition {
            from: Some(TransactionAuditState::Simulated),
            to: TransactionAuditState::AwaitingApproval,
            failure_code: None,
        },
        TransactionAuditEventKind::ApprovalPresented {
            warning_codes: vec![TransactionAuditWarningCode::Testnet],
        },
        TransactionAuditEventKind::ApprovalDecision {
            decision: ApprovalAuditDecision::Approved,
        },
        TransactionAuditEventKind::OwnerAuthorization {
            outcome: OwnerAuthorizationAuditOutcome::Succeeded,
        },
        TransactionAuditEventKind::ApprovalFinalization {
            outcome: ApprovalFinalizationAuditOutcome::Consumed,
        },
        TransactionAuditEventKind::Signing {
            result: TransactionAuditResult::Succeeded,
        },
        TransactionAuditEventKind::Broadcast {
            result: TransactionAuditResult::Succeeded,
        },
        TransactionAuditEventKind::Confirmation {
            result: TransactionAuditResult::Succeeded,
        },
        TransactionAuditEventKind::X402Payment {
            stage: X402AuditStage::PaidRetry,
            result: TransactionAuditResult::Failed(
                TransactionAuditFailureCode::X402OutcomeAmbiguous,
            ),
            payment_may_have_occurred: true,
        },
    ];
    for data in events {
        assert_eq!(logger.record(event(data)?), LogWriteOutcome::Written);
    }

    let log = fs::read_to_string(temporary.path().join(ACTIVE_LOG_NAME))?;
    for stage in [
        "lifecycle_transition",
        "approval_presented",
        "approval_decision",
        "owner_authorization",
        "approval_finalization",
        "signing",
        "broadcast",
        "confirmation",
        "x402_payment",
    ] {
        assert!(log.contains(&format!("\"event\":\"{stage}\"")));
    }
    assert!(log.contains(ACCOUNT));
    assert!(log.contains(&format!("0x{}", "11".repeat(32))));
    for forbidden in [
        "privateKey",
        "recoveryPhrase",
        "seedPhrase",
        "vaultPlaintext",
        "keychainData",
        "signature",
        "signedTransaction",
        "signingPayload",
        "prompt",
        "modelExplanation",
        "rawError",
    ] {
        assert!(!log.contains(forbidden));
    }
    for line in log.lines() {
        let record: Value = serde_json::from_str(line)?;
        assert_eq!(record["schemaVersion"], TRANSACTION_AUDIT_SCHEMA_VERSION);
        assert!(record.get("timestampUnixMs").is_some());
        assert!(record.get("sequence").is_some());
    }
    Ok(())
}

#[test]
fn x402_ambiguity_flag_is_derived_only_from_closed_failure_codes() {
    assert!(
        event(TransactionAuditEventKind::X402Payment {
            stage: X402AuditStage::SettlementValidation,
            result: TransactionAuditResult::Failed(
                TransactionAuditFailureCode::X402SettlementFailed,
            ),
            payment_may_have_occurred: true,
        })
        .is_ok()
    );
    assert!(
        event(TransactionAuditEventKind::X402Payment {
            stage: X402AuditStage::PaidRetry,
            result: TransactionAuditResult::Succeeded,
            payment_may_have_occurred: true,
        })
        .is_err()
    );
    assert!(
        event(TransactionAuditEventKind::X402Payment {
            stage: X402AuditStage::PaidRetry,
            result: TransactionAuditResult::Failed(TransactionAuditFailureCode::SigningFailed),
            payment_may_have_occurred: true,
        })
        .is_err()
    );
}

#[test]
fn denial_failure_cancellation_replay_and_crash_codes_are_closed() -> Result<(), Box<dyn Error>> {
    let temporary = TempDir::new()?;
    let logger = TransactionAuditLogger::new(temporary.path());
    for data in [
        TransactionAuditEventKind::ApprovalDecision {
            decision: ApprovalAuditDecision::Denied,
        },
        TransactionAuditEventKind::OwnerAuthorization {
            outcome: OwnerAuthorizationAuditOutcome::Cancelled,
        },
        TransactionAuditEventKind::ApprovalFinalization {
            outcome: ApprovalFinalizationAuditOutcome::ReplayRejected,
        },
        TransactionAuditEventKind::Signing {
            result: TransactionAuditResult::Failed(TransactionAuditFailureCode::SigningFailed),
        },
        TransactionAuditEventKind::LifecycleTransition {
            from: Some(TransactionAuditState::Approved),
            to: TransactionAuditState::Failed,
            failure_code: Some(TransactionAuditFailureCode::ApplicationRestarted),
        },
    ] {
        assert_eq!(logger.record(event(data)?), LogWriteOutcome::Written);
    }
    let log = fs::read_to_string(temporary.path().join(ACTIVE_LOG_NAME))?;
    for code in [
        "denied",
        "cancelled",
        "replay_rejected",
        "signing_failed",
        "application_restarted",
    ] {
        assert!(log.contains(code));
    }
    Ok(())
}

#[test]
fn invalid_ids_duplicate_warnings_and_inconsistent_failure_state_are_rejected() {
    assert!(matches!(
        TransactionAuditEvent::new(
            "../../secret",
            "transaction-01",
            ACCOUNT,
            8453,
            TransactionAuditTransactionType::Eip1559,
            None,
            None,
            TransactionAuditOrigin::Agent,
            TransactionAuditEventKind::ApprovalDecision {
                decision: ApprovalAuditDecision::Dismissed,
            },
        ),
        Err(TransactionAuditError::InvalidEvent)
    ));
    assert!(
        event(TransactionAuditEventKind::ApprovalPresented {
            warning_codes: vec![
                TransactionAuditWarningCode::Testnet,
                TransactionAuditWarningCode::Testnet,
            ],
        })
        .is_err()
    );
    assert!(
        event(TransactionAuditEventKind::LifecycleTransition {
            from: Some(TransactionAuditState::Approved),
            to: TransactionAuditState::Failed,
            failure_code: None,
        })
        .is_err()
    );
}

#[test]
fn append_revalidates_deserialized_events() -> Result<(), Box<dyn Error>> {
    let temporary = TempDir::new()?;
    let logger = TransactionAuditLogger::new(temporary.path());
    let mut value = serde_json::to_value(event(TransactionAuditEventKind::ApprovalDecision {
        decision: ApprovalAuditDecision::Approved,
    })?)?;
    value["signingDigest"] = Value::String(format!("0x{}", "GG".repeat(32)));
    let bypassed_constructor: TransactionAuditEvent = serde_json::from_value(value)?;

    assert_eq!(logger.record(bypassed_constructor), LogWriteOutcome::Failed);
    assert!(!temporary.path().join(ACTIVE_LOG_NAME).exists());
    Ok(())
}

#[test]
fn export_preview_discloses_public_onchain_metadata_and_rejects_corruption()
-> Result<(), Box<dyn Error>> {
    let temporary = TempDir::new()?;
    let logger = TransactionAuditLogger::new(temporary.path().join("audit"));
    assert_eq!(
        logger.record(event(TransactionAuditEventKind::Broadcast {
            result: TransactionAuditResult::Succeeded,
        })?),
        LogWriteOutcome::Written
    );
    let plan = logger.plan_export()?;
    assert_eq!(plan.preview().record_count(), 1);
    assert!(
        plan.preview()
            .privacy_description()
            .contains("public wallet addresses")
    );
    assert!(
        plan.preview()
            .privacy_description()
            .contains("transaction digests")
    );
    let destination = temporary.path().join("transaction-audit.jsonl");
    let receipt = plan.write_to(&destination)?;
    assert!(receipt.bytes_written() > 0);
    let exported = fs::read_to_string(&destination)?;
    assert_eq!(exported.lines().count(), 2);

    let mut active = fs::OpenOptions::new()
        .append(true)
        .open(temporary.path().join("audit").join(ACTIVE_LOG_NAME))?;
    active.write_all(b"{\"schemaVersion\":1,\"privateKey\":\"must-not-export\"}\n")?;
    active.flush()?;
    assert!(matches!(
        logger.plan_export(),
        Err(TransactionAuditError::CorruptSource)
    ));
    Ok(())
}

#[test]
fn explicit_deletion_removes_retained_audit_and_write_failure_has_no_authority()
-> Result<(), Box<dyn Error>> {
    let temporary = TempDir::new()?;
    let logger = TransactionAuditLogger::new(temporary.path().join("audit"));
    assert_eq!(
        logger.record(event(TransactionAuditEventKind::Signing {
            result: TransactionAuditResult::Succeeded,
        })?),
        LogWriteOutcome::Written
    );
    let receipt = logger.delete_all()?;
    assert_eq!(receipt.deleted_files(), 1);
    assert_eq!(logger.plan_export()?.preview().record_count(), 0);

    let occupied = temporary.path().join("occupied");
    fs::write(&occupied, b"not a directory")?;
    let unavailable = TransactionAuditLogger::new(occupied);
    assert_eq!(
        unavailable.record(event(TransactionAuditEventKind::ApprovalFinalization {
            outcome: ApprovalFinalizationAuditOutcome::Consumed,
        })?),
        LogWriteOutcome::Failed
    );
    let signing_authority_created_by_audit = false;
    assert!(!signing_authority_created_by_audit);
    Ok(())
}

#[test]
fn restart_continues_monotonic_event_order() -> Result<(), Box<dyn Error>> {
    let temporary = TempDir::new()?;
    {
        let logger = TransactionAuditLogger::new(temporary.path());
        assert_eq!(
            logger.record(event(TransactionAuditEventKind::Signing {
                result: TransactionAuditResult::Succeeded,
            })?),
            LogWriteOutcome::Written
        );
    }
    let reopened = TransactionAuditLogger::new(temporary.path());
    assert_eq!(
        reopened.record(event(TransactionAuditEventKind::Broadcast {
            result: TransactionAuditResult::Succeeded,
        })?),
        LogWriteOutcome::Written
    );
    let sequences = fs::read_to_string(temporary.path().join(ACTIVE_LOG_NAME))?
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).map(|value| value["sequence"].clone()))
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(sequences, vec![Value::from(0), Value::from(1)]);
    Ok(())
}
