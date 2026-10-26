use chain_evm::SimulationState;
use ipc_types::{EvmApprovalDecision, IpcErrorCode, TransactionProposalId};

use super::{ApprovalOrigin, EvmApprovalControl, presentation::simulation_label};

#[test]
fn origins_are_closed_host_authored_labels() {
    assert_eq!(
        ApprovalOrigin::User.display_name(),
        "User · transfer preparation screen"
    );
}

#[test]
fn every_simulation_outcome_has_an_explicit_prominent_label() {
    assert_eq!(
        simulation_label(SimulationState::Succeeded, false),
        "SUCCEEDED"
    );
    assert_eq!(
        simulation_label(SimulationState::Reverted, false),
        "REVERTED"
    );
    assert_eq!(
        simulation_label(SimulationState::Mismatch, false),
        "DISCREPANCY"
    );
    assert_eq!(
        simulation_label(SimulationState::Inconclusive, false),
        "INCONCLUSIVE"
    );
    assert_eq!(
        simulation_label(SimulationState::Unavailable, false),
        "UNAVAILABLE"
    );
    assert_eq!(simulation_label(SimulationState::Succeeded, true), "STALE");
}

#[test]
fn unknown_and_reused_review_references_fail_closed() -> Result<(), ipc_types::IpcError> {
    let control = EvmApprovalControl::new();
    let Err(error) = control.begin(&TransactionProposalId("not-issued-by-host".to_owned())) else {
        return Err(ipc_types::IpcError::internal());
    };
    assert_eq!(error.code, IpcErrorCode::InvalidRequest);
    assert!(!error.retryable);
    Ok(())
}

#[test]
fn a_concurrent_native_review_is_rejected_before_reference_resolution()
-> Result<(), ipc_types::IpcError> {
    let control = EvmApprovalControl::new();
    control
        .registry
        .lock()
        .map_err(|_| ipc_types::IpcError::internal())?
        .active = true;
    let Err(error) = control.begin(&TransactionProposalId("any-reference".to_owned())) else {
        return Err(ipc_types::IpcError::internal());
    };
    assert_eq!(error.code, IpcErrorCode::Unsupported);
    assert!(error.retryable);
    Ok(())
}

#[test]
fn frontend_decisions_are_closed_display_only_values() -> Result<(), serde_json::Error> {
    assert_eq!(
        serde_json::to_string(&EvmApprovalDecision::Approved)?,
        "\"approved\""
    );
    assert_eq!(
        serde_json::to_string(&EvmApprovalDecision::Denied)?,
        "\"denied\""
    );
    assert_eq!(
        serde_json::to_string(&EvmApprovalDecision::Dismissed)?,
        "\"dismissed\""
    );
    Ok(())
}

#[cfg(target_os = "macos")]
#[test]
fn native_surface_defaults_to_denial_and_keeps_approval_separate() {
    let source = include_str!("native.rs");
    let deny = source.find("addButtonWithTitle(&NSString::from_str(\"Deny\"))");
    let approve =
        source.find("addButtonWithTitle(&NSString::from_str(\"Approve exact transaction\"))");
    assert!(deny.is_some_and(|deny| approve.is_some_and(|approve| deny < approve)));
    assert!(source.contains("NSAlertFirstButtonReturn => EvmApprovalDecision::Denied"));
    assert!(source.contains("NSAlertSecondButtonReturn && display.approval_allowed"));
    assert!(source.contains("if display.approval_allowed"));
}
