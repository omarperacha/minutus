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
