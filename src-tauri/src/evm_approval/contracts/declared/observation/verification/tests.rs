//! Lifecycle reporting cannot infer inclusion from acquisition or error labels.
use super::*;
use crate::evm_approval::contracts::declared::lifecycle::Lifecycle;
use chain_evm::contract::generic::{GenericCompletionFailure, GenericError};
use serde_json::{Value, json};

#[test]
fn rejected_exact_inclusion_keeps_known_hash_without_inclusion_or_completion_credit()
-> Result<(), Box<dyn std::error::Error>> {
    // The chain owner separately tests genuine signed-body/signer/MPT mutations.
    // Its no-seal failure must stay pessimistic here, regardless of error label.
    for error in [
        GenericError::SnapshotMismatch,
        GenericError::ConditionUnsatisfied,
        GenericError::Expired,
    ] {
        let mut lifecycle = Lifecycle::default();
        let progress = lifecycle.step(0).ok_or("step")?;
        let hash = format!("0x{}", "a".repeat(64));
        progress
            .record_transaction_hash(&hash)
            .map_err(|error| error.message)?;
        progress.submission_acknowledged();
        progress.receipt_reported(chain_evm::TransactionLifecycle::Confirmed);
        progress.inclusion_started();
        ProofFailure::from(GenericCompletionFailure::Inclusion(error))
            .record_stage(progress)
            .map_err(|error| error.message)?;
        progress.observation_stopped(ObservationStop::InclusionUnavailable);
        let value = lifecycle
            .project_verification_unavailable(&IpcError::internal(), 0, &Value::Null)
            .ok_or("known submission lost")?;
        let step = &value["lifecycle"]["observations"]["steps"][0];
        assert_eq!(step["transactionHash"], hash);
        assert_eq!(step["postSubmission"]["receipt"], "successReported");
        assert_eq!(step["postSubmission"]["inclusionProof"], "unavailable");
        assert_eq!(step["postSubmission"]["completionProof"], "notStarted");
        assert_eq!(value["completedSteps"], 0);
        assert!(
            !value["hostOutcomeSummary"]
                .as_str()
                .ok_or("summary")?
                .contains("inclusion proof verified")
        );
    }
    Ok(())
}

#[test]
fn verified_inclusion_and_unavailable_completion_remain_distinct_without_task_credit()
-> Result<(), Box<dyn std::error::Error>> {
    // Projection of the seal-bearing stage. No opaque chain certificate is
    // fabricated: actual seal minting and later-error transport are tested in
    // chain-evm's completion::stages against real synthetic signed/MPT fixtures.
    let mut lifecycle = Lifecycle::default();
    let progress = lifecycle.step(0).ok_or("step")?;
    progress
        .record_transaction_hash(&format!("0x{}", "b".repeat(64)))
        .map_err(|error| error.message)?;
    progress.submission_acknowledged();
    progress.receipt_reported(chain_evm::TransactionLifecycle::Confirmed);
    progress.inclusion_verified();
    progress.completion_started();
    progress.observation_stopped(ObservationStop::CompletionUnavailable);
    let value = lifecycle
        .project_verification_unavailable(&IpcError::internal(), 0, &json!({}))
        .ok_or("partial result")?;
    let post = &value["lifecycle"]["observations"]["steps"][0]["postSubmission"];
    assert_eq!(post["inclusionProof"], "verified");
    assert_eq!(post["completionProof"], "unavailable");
    assert_eq!(value["state"], "submittedVerificationUnavailable");
    assert_eq!(value["completedSteps"], 0);
    assert_eq!(value["retryAutomatically"], false);
    Ok(())
}
