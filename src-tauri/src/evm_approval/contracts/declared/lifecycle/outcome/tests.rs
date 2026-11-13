use super::super::MAX_STEPS;
use super::*;
use ipc_types::{EvmApprovalDecision, IpcError, IpcErrorCode, WalletAvailability};

fn ready() -> Lifecycle {
    let mut lifecycle = Lifecycle::default();
    lifecycle.wallet_readiness_requested(WalletAvailability::Locked);
    lifecycle.wallet_ready();
    lifecycle.scope_review_requested();
    lifecycle.scope_decision(EvmApprovalDecision::Approved);
    lifecycle.source_preflight_succeeded();
    lifecycle
}
fn failed(
    lifecycle: &Lifecycle,
    code: IpcErrorCode,
    prose: &str,
) -> Result<Value, serde_json::Error> {
    let error = lifecycle.project_error(
        IpcError {
            code,
            message: prose.into(),
            retryable: true,
        },
        0,
        &Value::Null,
    );
    serde_json::from_str(&error.message)
}

#[test]
fn fresh_authentication_stop_preserves_successful_initial_unlock_and_exact_approval()
-> Result<(), Box<dyn std::error::Error>> {
    let mut lifecycle = ready();
    let step = lifecycle.step(0).ok_or("step")?;
    step.exact_review_decision(EvmApprovalDecision::Approved);
    step.authorization_requested();
    let result = failed(
        &lifecycle,
        IpcErrorCode::Cancelled,
        "Wallet operation was cancelled",
    )?;
    let text = result["hostOutcomeSummary"].as_str().ok_or("summary")?;
    for fact in [
        "Initial wallet unlock succeeded; the initial unlock was not cancelled.",
        "Outbound data-access consent was approved.",
        "Source preflight succeeded.",
        "Exact transaction review was approved.",
        "Fresh signing authentication was requested; no successful authentication was recorded.",
        "Signing was not started.",
        "Submission was not attempted.",
        "Current wallet lock state was not observed.",
    ] {
        assert!(text.contains(fact), "{fact}");
    }
    assert_eq!(
        result["latestObservedStage"],
        json!({"stage":"signingAuthenticationRequested","stepIndex":0})
    );
    assert_eq!(result["currentWalletState"], "notObserved");
    assert_eq!(result["retryAutomatically"], false);
    Ok(())
}

#[test]
fn generic_error_code_and_prose_cannot_assign_an_authentication_cause_or_current_lock_state()
-> Result<(), Box<dyn std::error::Error>> {
    let mut lifecycle = ready();
    lifecycle.step(0).ok_or("step")?.authorization_requested();
    let expected = failed(&lifecycle, IpcErrorCode::Cancelled, "cancelled")?;
    for (code, prose) in [
        (
            IpcErrorCode::PermissionDenied,
            "Password wrong; wallet locked",
        ),
        (IpcErrorCode::Internal, "untrusted diagnostic text"),
    ] {
        let result = failed(&lifecycle, code, prose)?;
        assert_eq!(result["hostOutcomeSummary"], expected["hostOutcomeSummary"]);
        assert_eq!(
            result["latestObservedStage"],
            expected["latestObservedStage"]
        );
        assert_eq!(result["currentWalletState"], "notObserved");
        assert_eq!(result["message"], prose);
    }
    Ok(())
}

#[test]
fn actual_review_decisions_and_initial_readiness_stop_remain_distinct()
-> Result<(), Box<dyn std::error::Error>> {
    let mut lifecycle = Lifecycle::default();
    lifecycle.wallet_readiness_requested(WalletAvailability::Locked);
    assert_eq!(
        json!(latest(&lifecycle))["stage"],
        "walletReadinessRequested"
    );
    assert!(summary(&lifecycle).contains("no successful readiness completion was recorded"));
    lifecycle.wallet_ready();
    lifecycle.scope_decision(EvmApprovalDecision::Denied);
    assert_eq!(json!(latest(&lifecycle))["stage"], "scopeReviewDenied");
    assert!(summary(&lifecycle).contains("Outbound data-access consent was denied."));
    lifecycle
        .step(0)
        .ok_or("step")?
        .exact_review_decision(EvmApprovalDecision::Dismissed);
    assert_eq!(json!(latest(&lifecycle))["stage"], "exactReviewDismissed");
    assert!(summary(&lifecycle).contains("Exact transaction review was dismissed."));
    assert!(!summary(&lifecycle).contains("Fresh signing authentication succeeded."));
    Ok(())
}

#[test]
fn uncertain_signing_and_acknowledged_submission_cannot_be_reported_as_unsent()
-> Result<(), Box<dyn std::error::Error>> {
    let mut lifecycle = ready();
    lifecycle.step(0).ok_or("step")?.signing_started();
    let result = failed(&lifecycle, IpcErrorCode::Internal, "signing error")?;
    assert!(
        result["hostOutcomeSummary"]
            .as_str()
            .ok_or("summary")?
            .contains("Signing started; whether a signature was produced is unknown.")
    );
    assert_eq!(
        result["latestObservedStage"]["stage"],
        "signingStartedOutcomeUnknown"
    );
    let step = lifecycle.step(0).ok_or("step")?;
    step.signature_returned();
    step.submission_acknowledged();
    lifecycle
        .step(1)
        .ok_or("step")?
        .submission_may_have_been_sent();
    let result = failed(&lifecycle, IpcErrorCode::Internal, "journal error")?;
    let text = result["hostOutcomeSummary"].as_str().ok_or("summary")?;
    assert!(text.contains("Submission was acknowledged;"));
    assert!(text.contains("Submission may have been sent; delivery is uncertain."));
    assert_eq!(result["completedSteps"], 0);
    assert_eq!(
        result["latestObservedStage"],
        json!({"stage":"submissionMayHaveBeenSent","stepIndex":1})
    );
    Ok(())
}

#[test]
fn before_write_reservation_covers_longest_summary_fragments_and_json_quoting()
-> Result<(), Box<dyn std::error::Error>> {
    let mut lifecycle = Lifecycle::default();
    lifecycle.wallet_readiness_requested(WalletAvailability::Locked);
    lifecycle.scope_review_requested();
    lifecycle.source_preflight_started();
    for index in 0..MAX_STEPS {
        let step = lifecycle.step(index).ok_or("step")?;
        step.exact_review_requested();
        step.authorization_requested();
        step.signing_started();
        step.presend_refresh_started();
        step.submission_acknowledged();
        step.record_transaction_hash(&format!("0x{}", "f".repeat(64)))
            .map_err(|error| error.message)?;
    }
    let result = failed(&lifecycle, IpcErrorCode::Internal, "bounded native failure")?;
    let encoded = json!({"error":result.to_string(),"retryable":false}).to_string();
    assert!(
        encoded.len()
            <= super::super::maximum_projection_bytes(MAX_STEPS).ok_or("invalid step bound")?
    );
    assert_eq!(
        result["lifecycle"]["observations"]["steps"]
            .as_array()
            .map(Vec::len),
        Some(MAX_STEPS)
    );
    assert!(
        result["hostOutcomeSummary"]
            .as_str()
            .ok_or("summary")?
            .contains("Step 16:")
    );
    Ok(())
}
