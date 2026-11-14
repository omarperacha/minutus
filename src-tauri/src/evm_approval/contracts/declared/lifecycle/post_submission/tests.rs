use super::super::{
    Lifecycle, MAX_AUTHORIZATION_EVENTS, MAX_STEPS, authorization, maximum_projection_bytes,
};
use super::*;

fn acknowledged(lifecycle: &mut Lifecycle) -> Result<&mut StepLifecycle, &'static str> {
    let step = lifecycle.step(0).ok_or("step")?;
    step.record_transaction_hash(&format!("0x{}", "a".repeat(64)))
        .map_err(|_| "hash")?;
    step.submission_acknowledged();
    Ok(step)
}

#[test]
fn acknowledged_but_unverified_is_a_stopped_partial_observation_not_failed_execution()
-> Result<(), Box<dyn std::error::Error>> {
    let mut lifecycle = Lifecycle::default();
    let step = acknowledged(&mut lifecycle)?;
    step.receipt_wait_started();
    step.observation_stopped(ObservationStop::BudgetElapsed);
    let partial = json!({"historicalCompletions":[]});
    let value = lifecycle
        .project_verification_unavailable(&IpcError::internal(), 0, &partial)
        .ok_or("partial result missing")?;
    assert_eq!(value["state"], "submittedVerificationUnavailable");
    assert_eq!(value["completedSteps"], 0);
    assert_eq!(value["partialFacts"], partial);
    assert_eq!(value["retryAutomatically"], false);
    let step = &value["lifecycle"]["observations"]["steps"][0];
    assert_eq!(step["submission"], "acknowledged");
    assert_eq!(step["postSubmission"]["receipt"], "unavailable");
    assert_eq!(step["transactionHash"], format!("0x{}", "a".repeat(64)));
    assert!(
        value["hostOutcomeSummary"]
            .as_str()
            .ok_or("summary")?
            .contains("transaction outcome is not established")
    );
    Ok(())
}

#[test]
fn raw_receipt_success_is_not_inclusion_or_completion_and_survives_later_stop()
-> Result<(), Box<dyn std::error::Error>> {
    let mut lifecycle = Lifecycle::default();
    let step = acknowledged(&mut lifecycle)?;
    step.receipt_reported(TransactionLifecycle::Confirmed);
    step.inclusion_started();
    step.observation_stopped(ObservationStop::InclusionUnavailable);
    let value = lifecycle
        .project_verification_unavailable(&IpcError::internal(), 0, &Value::Null)
        .ok_or("partial")?;
    let post = &value["lifecycle"]["observations"]["steps"][0]["postSubmission"];
    assert_eq!(post["receipt"], "successReported");
    assert_eq!(post["inclusionProof"], "unavailable");
    assert_eq!(post["completionProof"], "notStarted");
    let mut context_stop = Lifecycle::default();
    let step = acknowledged(&mut context_stop)?;
    step.receipt_reported(TransactionLifecycle::Confirmed);
    step.observation_stopped(ObservationStop::ContextUnavailable);
    assert_eq!(
        context_stop.facts()["observations"]["steps"][0]["postSubmission"]["receipt"],
        "successReported"
    );
    Ok(())
}

#[test]
fn no_send_or_known_unsatisfied_completion_cannot_mint_an_unavailable_result()
-> Result<(), Box<dyn std::error::Error>> {
    let mut no_send = Lifecycle::default();
    no_send
        .step(0)
        .ok_or("step")?
        .observation_stopped(ObservationStop::MonitorRejected);
    assert!(
        no_send
            .project_verification_unavailable(&IpcError::internal(), 0, &Value::Null)
            .is_none()
    );
    let mut unsatisfied = Lifecycle::default();
    let step = acknowledged(&mut unsatisfied)?;
    step.receipt_reported(TransactionLifecycle::Confirmed);
    step.inclusion_verified();
    step.completion_unsatisfied();
    assert!(
        unsatisfied
            .project_verification_unavailable(&IpcError::internal(), 0, &Value::Null)
            .is_none()
    );
    assert_eq!(
        unsatisfied.facts()["observations"]["steps"][0]["postSubmission"]["completionProof"],
        "unsatisfied"
    );
    Ok(())
}

#[test]
fn monitor_attempts_and_complete_report_remain_bounded_for_every_admitted_size()
-> Result<(), Box<dyn std::error::Error>> {
    for count in [1, 8, MAX_STEPS] {
        let mut lifecycle = Lifecycle::default();
        for index in 0..count {
            let step = lifecycle.step(index).ok_or("step")?;
            step.submission_acknowledged();
            step.record_transaction_hash(&format!("0x{}", "f".repeat(64)))
                .map_err(|error| error.message)?;
            for _ in 0..MAX_AUTHORIZATION_EVENTS {
                step.record_authorization_event(authorization::largest_event())
                    .map_err(|error| error.message)?;
            }
            for _ in 0..MAX_MONITOR_ATTEMPTS {
                step.receipt_attempted().map_err(|error| error.message)?;
            }
            assert!(step.receipt_attempted().is_err());
            step.receipt_reported(TransactionLifecycle::Confirmed);
            step.inclusion_verified();
            step.completion_started();
            step.observation_stopped(ObservationStop::CompletionUnavailable);
        }
        let value = lifecycle
            .project_verification_unavailable(&IpcError::internal(), 0, &Value::Null)
            .ok_or("partial")?;
        let encoded = json!({"error":value.to_string(),"retryable":false}).to_string();
        assert!(encoded.len() <= maximum_projection_bytes(count).ok_or("bound")?);
    }
    Ok(())
}
