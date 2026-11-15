//! Persisted send/inclusion state never becomes a new write capability.
use super::super::lifecycle::{ObservationStop, StepLifecycle};
use app_core::{ContractStepState, PreparedDeclaredContractStep, StoredEvmTransactionState};
use chain_evm::TransactionLifecycle;
use ipc_types::IpcError;

pub(super) fn stopped(
    step: &PreparedDeclaredContractStep,
    progress: &mut StepLifecycle,
    hash: &str,
    reason: ObservationStop,
    message: &str,
) -> IpcError {
    progress.observation_stopped(reason);
    let submitted = step.record_execution(
        ContractStepState::Submitted,
        ContractStepState::Uncertain,
        None,
    );
    if !matches!(submitted, Ok(true)) {
        let _ = step.record_execution(
            ContractStepState::ConfirmedUnverified,
            ContractStepState::Uncertain,
            None,
        );
    }
    IpcError::invalid_request(format!(
        "{message}; known transaction {hash}; submission remains known; no automatic retry or cleanup"
    ))
}

pub(super) fn terminal_status(
    lifecycle: TransactionLifecycle,
) -> Result<(StoredEvmTransactionState, ContractStepState), IpcError> {
    match lifecycle {
        TransactionLifecycle::Confirmed => Ok((
            StoredEvmTransactionState::Confirmed,
            ContractStepState::ConfirmedUnverified,
        )),
        TransactionLifecycle::Failed => {
            Ok((StoredEvmTransactionState::Failed, ContractStepState::Failed))
        }
        TransactionLifecycle::DroppedOrReplaced => Ok((
            StoredEvmTransactionState::DroppedOrReplaced,
            ContractStepState::Uncertain,
        )),
        TransactionLifecycle::Pending => Err(IpcError::internal()),
    }
}
