use super::*;
use crate::evm_approval::contracts::declared::lifecycle::{
    Lifecycle, MAX_AUTHORIZATION_EVENTS, MAX_STEPS, maximum_projection_bytes,
};
use ipc_types::IpcError;
use serde_json::{Value, json};

fn result(lifecycle: &Lifecycle) -> Result<Value, serde_json::Error> {
    serde_json::from_str(
        &lifecycle
            .project_error(IpcError::internal(), 0, &Value::Null)
            .message,
    )
}

#[test]
fn accepted_password_does_not_claim_authentication_but_typed_success_does()
-> Result<(), Box<dyn std::error::Error>> {
    let mut lifecycle = Lifecycle::default();
    let step = lifecycle.step(0).ok_or("step")?;
    step.authorization_requested();
    step.record_authorization_event(WalletAuthorizationEvent::PasswordDecision {
        purpose: WalletPasswordPurpose::SignTransaction,
        decision: WalletPasswordDecision::Accepted,
    })
    .map_err(|error| error.message)?;
    let before = result(&lifecycle)?;
    assert_eq!(
        before["lifecycle"]["observations"]["steps"][0]["authorization"],
        "requested"
    );
    assert!(
        before["hostOutcomeSummary"]
            .as_str()
            .ok_or("summary")?
            .contains("acceptance alone does not establish successful authentication")
    );
    lifecycle
        .step(0)
        .ok_or("step")?
        .record_authorization_event(WalletAuthorizationEvent::AuthenticationSucceeded)
        .map_err(|error| error.message)?;
    let after = result(&lifecycle)?;
    assert_eq!(
        after["lifecycle"]["observations"]["steps"][0]["authorization"],
        "authenticated"
    );
    assert_eq!(
        after["lifecycle"]["observations"]["steps"][0]["signing"],
        "notStarted"
    );
    assert_eq!(
        after["latestObservedStage"]["stage"],
        "signingAuthenticationSucceeded"
    );
    Ok(())
}

#[test]
fn actual_cancel_button_host_abort_and_credential_rejection_remain_distinct()
-> Result<(), Box<dyn std::error::Error>> {
    for (event, phrase) in [
        (
            WalletAuthorizationEvent::PasswordDecision {
                purpose: WalletPasswordPurpose::SignTransaction,
                decision: WalletPasswordDecision::CancelButton,
            },
            "Cancel button was selected",
        ),
        (
            WalletAuthorizationEvent::PasswordDecision {
                purpose: WalletPasswordPurpose::SignTransaction,
                decision: WalletPasswordDecision::HostAbort,
            },
            "host aborted the native password dialog",
        ),
        (
            WalletAuthorizationEvent::AuthenticationFailed {
                failure: WalletAuthenticationFailure::CredentialsRejected,
            },
            "credentials rejected",
        ),
        (
            WalletAuthorizationEvent::AuthenticationFailed {
                failure: WalletAuthenticationFailure::Cancelled,
            },
            "does not establish a user Cancel click",
        ),
    ] {
        let mut lifecycle = Lifecycle::default();
        let step = lifecycle.step(0).ok_or("step")?;
        step.authorization_requested();
        step.record_authorization_event(event)
            .map_err(|error| error.message)?;
        let projected = result(&lifecycle)?;
        assert!(
            projected["hostOutcomeSummary"]
                .as_str()
                .ok_or("summary")?
                .contains(phrase)
        );
        assert_eq!(
            projected["lifecycle"]["observations"]["steps"][0]["authorization"],
            "requested"
        );
        assert_eq!(projected["currentWalletState"], "notObserved");
    }
    Ok(())
}

#[test]
fn later_job_invalidation_keeps_the_actual_native_password_decision()
-> Result<(), Box<dyn std::error::Error>> {
    let mut lifecycle = Lifecycle::default();
    let step = lifecycle.step(0).ok_or("step")?;
    step.authorization_requested();
    for event in [
        WalletAuthorizationEvent::PasswordDecision {
            purpose: WalletPasswordPurpose::SignTransaction,
            decision: WalletPasswordDecision::HostAbort,
        },
        WalletAuthorizationEvent::JobInvalidated {
            stage: WalletAuthorizationStage::AfterPassword,
        },
    ] {
        step.record_authorization_event(event)
            .map_err(|error| error.message)?;
    }
    step.record_request_cancellation(TransferCancellationReason::WalletEpochChanged);
    step.record_request_cancellation(TransferCancellationReason::Interrupted);
    let projected = result(&lifecycle)?;
    let summary = projected["hostOutcomeSummary"].as_str().ok_or("summary")?;
    assert!(summary.contains("host aborted the native password dialog"));
    assert!(summary.contains("job was invalidated after the native password dialog returned"));
    assert!(summary.contains("observed a wallet epoch change"));
    assert_eq!(
        projected["lifecycle"]["observations"]["steps"][0]["requestCancellation"],
        "walletEpochChanged"
    );
    assert_eq!(
        projected["latestObservedStage"]["stage"],
        "authorizationJobInvalidated"
    );
    Ok(())
}

#[test]
fn authorization_observations_have_a_strict_bound_without_silent_replacement()
-> Result<(), Box<dyn std::error::Error>> {
    let mut lifecycle = Lifecycle::default();
    let step = lifecycle.step(0).ok_or("step")?;
    step.authorization_requested();
    for _ in 0..MAX_AUTHORIZATION_EVENTS {
        step.record_authorization_event(largest_event())
            .map_err(|error| error.message)?;
    }
    assert!(
        step.record_authorization_event(WalletAuthorizationEvent::AuthenticationSucceeded)
            .is_err()
    );
    let projected = result(&lifecycle)?;
    assert_eq!(
        projected["lifecycle"]["observations"]["steps"][0]["authorizationEvents"]
            .as_array()
            .map(Vec::len),
        Some(MAX_AUTHORIZATION_EVENTS)
    );
    assert_eq!(
        projected["lifecycle"]["observations"]["steps"][0]["authorization"],
        "requested"
    );
    Ok(())
}

#[test]
fn reservation_covers_each_declared_size_including_sixteen_full_event_lists()
-> Result<(), Box<dyn std::error::Error>> {
    for count in [1, 8, MAX_STEPS] {
        let mut lifecycle = Lifecycle::default();
        lifecycle.record_request_cancellation(TransferCancellationReason::WalletEpochChanged);
        lifecycle.record_request_cancellation(TransferCancellationReason::Interrupted);
        for index in 0..count {
            let step = lifecycle.step(index).ok_or("step")?;
            step.authorization_requested();
            for _ in 0..MAX_AUTHORIZATION_EVENTS {
                step.record_authorization_event(largest_event())
                    .map_err(|error| error.message)?;
            }
            step.record_request_cancellation(TransferCancellationReason::WalletEpochChanged);
            step.record_transaction_hash(&format!("0x{}", "f".repeat(64)))
                .map_err(|error| error.message)?;
        }
        let projected = result(&lifecycle)?;
        assert_eq!(
            projected["lifecycle"]["observations"]["requestCancellation"],
            "walletEpochChanged"
        );
        let encoded = json!({"message":projected.to_string()}).to_string();
        assert!(encoded.len() <= maximum_projection_bytes(count).ok_or("invalid step bound")?);
    }
    let maximum_size = serde_json::to_vec(&largest_event())?.len();
    for event in [
        WalletAuthorizationEvent::PasswordRequested {
            purpose: WalletPasswordPurpose::OtherWalletOperation,
        },
        WalletAuthorizationEvent::PasswordPresented {
            purpose: WalletPasswordPurpose::OtherWalletOperation,
        },
        WalletAuthorizationEvent::JobInvalidated {
            stage: WalletAuthorizationStage::BeforePassword,
        },
        WalletAuthorizationEvent::AuthenticationStarted,
        WalletAuthorizationEvent::AuthenticationSucceeded,
        WalletAuthorizationEvent::AuthenticationFailed {
            failure: WalletAuthenticationFailure::CredentialsRejected,
        },
    ] {
        assert!(serde_json::to_vec(&event)?.len() <= maximum_size);
    }
    Ok(())
}
