use super::*;
#[test]
fn pre_send_revocation_is_known_unsent_but_a_transport_failure_after_admission_is_unknown() {
    let cancelled = broadcast_failure(chain_evm::BroadcastError::SubmissionCancelled, false);
    assert_eq!(cancelled.code, IpcErrorCode::Cancelled);
    assert!(cancelled.message.contains("before submission"));
    assert!(!cancelled.retryable);
    let unknown = broadcast_failure(chain_evm::BroadcastError::Transport, true);
    assert!(unknown.message.contains("may have been sent"));
    assert!(!unknown.retryable);
    let unsent = broadcast_failure(chain_evm::BroadcastError::Transport, false);
    assert!(!unsent.message.contains("may have been sent"));
    assert!(!unsent.retryable);
}

#[test]
fn a_panicked_callback_never_becomes_false_admission_evidence() {
    assert!(!possible_submission(false, false, false));
    assert!(!possible_submission(false, true, true));
    assert!(possible_submission(false, true, false));
    assert!(possible_submission(true, true, false));
    assert!(possible_submission(true, true, true));
    let error = broadcast_failure(
        chain_evm::BroadcastError::Transport,
        possible_submission(false, true, false),
    );
    assert!(error.message.contains("may have been sent"));
    assert!(!error.retryable);
}
