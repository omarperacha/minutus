use super::*;
use ipc_types::IpcErrorCode;

fn source_error() -> IpcError {
    IpcError::invalid_request("SourcePreflight: MissingState")
}
fn projected(lifecycle: &Lifecycle) -> Result<Value, serde_json::Error> {
    serde_json::from_str(
        &lifecycle
            .project_error(source_error(), 0, &Value::Null)
            .message,
    )
}

#[test]
fn initial_unlock_and_outbound_approval_are_not_exact_transaction_approval()
-> Result<(), Box<dyn std::error::Error>> {
    let mut lifecycle = Lifecycle::default();
    lifecycle.wallet_readiness_requested(WalletAvailability::Locked);
    lifecycle.wallet_ready();
    lifecycle.scope_review_requested();
    lifecycle.scope_decision(EvmApprovalDecision::Approved);
    lifecycle.source_preflight_started();
    let result = projected(&lifecycle)?;
    let observations = &result["lifecycle"]["observations"];
    assert_eq!(observations["walletReadiness"]["unlockRequired"], true);
    assert_eq!(
        observations["walletReadiness"]["initialAvailability"],
        "locked"
    );
    assert_eq!(observations["walletReadiness"]["status"], "succeeded");
    assert_eq!(observations["scopeReview"], "approved");
    assert_eq!(observations["sourcePreflight"], "started");
    assert_eq!(observations["steps"], json!([]));
    assert!(
        result["lifecycle"]["promptVisibility"]
            .as_str()
            .is_some_and(|text| text.starts_with("notRecorded"))
    );
    Ok(())
}

#[test]
fn missing_state_after_authentication_does_not_erase_exact_approval_or_invent_signing()
-> Result<(), Box<dyn std::error::Error>> {
    let mut lifecycle = Lifecycle::default();
    lifecycle.source_preflight_succeeded();
    let step = lifecycle.step(0).ok_or("bounded step missing")?;
    step.exact_review_requested();
    step.exact_review_decision(EvmApprovalDecision::Approved);
    step.authorization_requested();
    step.authentication_succeeded();
    let result = projected(&lifecycle)?;
    let step = &result["lifecycle"]["observations"]["steps"][0];
    assert_eq!(step["exactReview"], "approved");
    assert_eq!(step["authorization"], "authenticated");
    assert_eq!(step["signing"], "notStarted");
    assert_eq!(step["submission"], "notAttempted");
    Ok(())
}

#[test]
fn signing_error_is_unknown_and_later_refresh_failure_retains_returned_signature()
-> Result<(), Box<dyn std::error::Error>> {
    let mut lifecycle = Lifecycle::default();
    lifecycle.step(0).ok_or("step missing")?.signing_started();
    assert_eq!(
        projected(&lifecycle)?["lifecycle"]["observations"]["steps"][0]["signing"],
        "startedOutcomeUnknown"
    );
    let step = lifecycle.step(0).ok_or("same step missing")?;
    step.signature_returned();
    step.presend_refresh_started();
    let result = projected(&lifecycle)?;
    let step = &result["lifecycle"]["observations"]["steps"][0];
    assert_eq!(step["signing"], "signatureReturned");
    assert_eq!(step["presendRefresh"], "started");
    assert_eq!(step["submission"], "notAttempted");
    Ok(())
}

#[test]
fn acknowledgement_survives_later_error_and_uncertainty_never_claims_unsent()
-> Result<(), Box<dyn std::error::Error>> {
    let mut lifecycle = Lifecycle::default();
    let uncertain_hash = format!("0x{}", "a".repeat(64));
    let acknowledged_hash = format!("0x{}", "b".repeat(64));
    lifecycle
        .step(0)
        .ok_or("step missing")?
        .record_transaction_hash(&uncertain_hash)
        .map_err(|error| error.message)?;
    lifecycle
        .step(1)
        .ok_or("step missing")?
        .record_transaction_hash(&acknowledged_hash)
        .map_err(|error| error.message)?;
    lifecycle
        .step(0)
        .ok_or("step missing")?
        .submission_may_have_been_sent();
    lifecycle
        .step(1)
        .ok_or("step missing")?
        .submission_acknowledged();
    lifecycle
        .step(1)
        .ok_or("step missing")?
        .submission_may_have_been_sent();
    let partial = json!({"knownHash":"public"});
    let original = IpcError {
        code: IpcErrorCode::Internal,
        message: "Submission acknowledged but journal update failed".into(),
        retryable: true,
    };
    let error = lifecycle.project_error(original, 0, &partial);
    assert_eq!(error.code, IpcErrorCode::Internal);
    assert!(!error.retryable);
    let result: Value = serde_json::from_str(&error.message)?;
    assert_eq!(
        result["message"],
        "Submission acknowledged but journal update failed"
    );
    assert_eq!(result["partialFacts"], partial);
    assert_eq!(result["completedSteps"], 0);
    let steps = &result["lifecycle"]["observations"]["steps"];
    assert_eq!(steps[0]["submission"], "mayHaveBeenSent");
    assert_eq!(steps[1]["submission"], "acknowledged");
    assert_eq!(steps[0]["transactionHash"], uncertain_hash);
    assert_eq!(steps[1]["transactionHash"], acknowledged_hash);
    assert_eq!(result["retryAutomatically"], false);
    assert_eq!(result["automaticCleanup"], false);
    Ok(())
}

#[test]
fn success_preserves_result_and_bounded_prior_step_facts() -> Result<(), Box<dyn std::error::Error>>
{
    let mut lifecycle = Lifecycle::default();
    for index in 0..MAX_STEPS {
        let step = lifecycle.step(index).ok_or("admitted maximum missing")?;
        step.exact_review_decision(EvmApprovalDecision::Dismissed);
        step.authentication_succeeded();
        step.signing_started();
        step.presend_refresh_succeeded();
        step.submission_may_have_been_sent();
        step.record_transaction_hash(&format!("0x{}", "f".repeat(64)))
            .map_err(|error| error.message)?;
    }
    assert!(lifecycle.step(MAX_STEPS).is_none());
    let result = lifecycle.project_success(json!({"state":"executionConfirmed"}));
    assert_eq!(result["state"], "executionConfirmed");
    assert_eq!(
        result["lifecycle"]["observations"]["steps"]
            .as_array()
            .map(Vec::len),
        Some(MAX_STEPS)
    );
    let encoded = json!({"message":result.to_string()}).to_string();
    assert!(encoded.len() <= maximum_projection_bytes(MAX_STEPS).ok_or("invalid step bound")?);
    Ok(())
}

#[test]
fn immutable_step_count_sizes_the_complete_budget_without_widening_the_maximum()
-> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(maximum_projection_bytes(0), None);
    assert_eq!(maximum_projection_bytes(MAX_STEPS + 1), None);
    let mut previous = 0;
    for count in 1..=MAX_STEPS {
        let bound = maximum_projection_bytes(count).ok_or("valid bound missing")?;
        assert!(bound > previous);
        previous = bound;
    }
    let eight = maximum_projection_bytes(8).ok_or("eight-step bound missing")?;
    let sixteen = maximum_projection_bytes(MAX_STEPS).ok_or("maximum bound missing")?;
    assert!(eight < sixteen);
    Ok(())
}

#[test]
fn unavailable_wallet_does_not_claim_unlock_is_required() -> Result<(), Box<dyn std::error::Error>>
{
    let mut lifecycle = Lifecycle::default();
    lifecycle.wallet_readiness_requested(WalletAvailability::Unavailable);
    let result = projected(&lifecycle)?;
    let readiness = &result["lifecycle"]["observations"]["walletReadiness"];
    assert_eq!(readiness["status"], "started");
    assert_eq!(readiness["initialAvailability"], "unavailable");
    assert_eq!(readiness["unlockRequired"], false);
    Ok(())
}

#[test]
fn transaction_identifier_rejects_malformed_oversized_and_changed_values()
-> Result<(), Box<dyn std::error::Error>> {
    let mut lifecycle = Lifecycle::default();
    let step = lifecycle.step(0).ok_or("step missing")?;
    for invalid in [
        String::new(),
        format!("0x{}", "f".repeat(65)),
        format!("0x{}", "A".repeat(64)),
        format!("0x{}g", "f".repeat(63)),
        format!("0X{}", "f".repeat(64)),
        format!("0x{}", "f".repeat(63)),
    ] {
        assert!(step.record_transaction_hash(&invalid).is_err());
    }
    let hash = format!("0x{}", "f".repeat(64));
    step.record_transaction_hash(&hash)
        .map_err(|error| error.message)?;
    step.record_transaction_hash(&hash)
        .map_err(|error| error.message)?;
    assert!(
        step.record_transaction_hash(&format!("0x{}", "a".repeat(64)))
            .is_err()
    );
    assert_eq!(
        projected(&lifecycle)?["lifecycle"]["observations"]["steps"][0]["transactionHash"],
        hash
    );
    Ok(())
}
