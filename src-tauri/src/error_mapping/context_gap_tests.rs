use super::{AppCoreError, IpcErrorCode, generation_error};

#[test]
fn mandatory_context_gap_explains_resolution_without_requesting_identical_retry() {
    let error = generation_error(AppCoreError::WorkingContext(
        agent_core::WorkingContextError::ContextCapacityGap,
    ));
    assert_eq!(error.code, IpcErrorCode::Unsupported);
    assert!(!error.retryable);
    assert!(error.message.contains("larger context"));
    assert!(error.message.contains("retained for recall"));
}
