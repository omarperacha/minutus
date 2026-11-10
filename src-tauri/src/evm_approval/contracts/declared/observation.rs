//! Canonical inclusion and declared state outcomes remain separate from receipt status.
use super::context::{Context, error};
use crate::wallet_lifecycle::{TransferCancellation, WalletControl};
use app_core::{
    AppCore, ContractStepState, DeclaredContractCompletion, PreparedDeclaredContractStep,
    StoredEvmTransaction, StoredEvmTransactionState,
};
use chain_evm::{
    TransactionHash, TransactionLifecycle,
    contract::{
        acquire_contract_transaction_proof,
        generic::{
            GenericError, VerifiedGenericCompletion, VerifiedGenericPrerequisites,
            verify_generic_completion,
        },
    },
    monitor_transaction,
};
use ipc_types::IpcError;
use std::sync::Arc;

pub(super) struct CompletedStep {
    pub(super) step: Arc<PreparedDeclaredContractStep>,
    pub(super) completion: Arc<DeclaredContractCompletion>,
}
pub(super) enum ObservationFailure {
    Unsatisfied,
    Unknown(IpcError),
}
impl From<GenericError> for ObservationFailure {
    fn from(failure: GenericError) -> Self {
        if failure == GenericError::ConditionUnsatisfied {
            Self::Unsatisfied
        } else {
            Self::Unknown(error(failure))
        }
    }
}
impl From<IpcError> for ObservationFailure {
    fn from(error: IpcError) -> Self {
        Self::Unknown(error)
    }
}
pub(super) async fn prerequisites(
    context: &Context,
    step: &PreparedDeclaredContractStep,
    completed: &[CompletedStep],
) -> Result<VerifiedGenericPrerequisites, IpcError> {
    context
        .prerequisites(
            step,
            &completed
                .iter()
                .map(|step| Arc::clone(&step.completion))
                .collect::<Vec<_>>(),
        )
        .await
}
async fn prove(
    context: &Context,
    step: Arc<PreparedDeclaredContractStep>,
    hash: String,
) -> Result<VerifiedGenericCompletion, ObservationFailure> {
    let network = context.network;
    let endpoint = Arc::clone(&context.endpoint);
    let cancelled = Arc::clone(&context.cancelled);
    let identities = context.identity_views();
    let prepared = context
        .candidate_views()
        .map_err(ObservationFailure::Unknown)?;
    let conditions = step
        .completion_conditions()
        .map_err(|error| crate::error_mapping::app_core_error(error.into()))?;
    tokio::task::spawn_blocking(move || {
        let inclusion = acquire_contract_transaction_proof(network, &endpoint, &hash, &cancelled)
            .map_err(|_| GenericError::SnapshotMismatch)?;
        let mut identities = identities.iter().map(Arc::as_ref).collect::<Vec<_>>();
        for previous in &prepared {
            if !identities
                .iter()
                .any(|identity| identity.commitment() == previous.identity().commitment())
            {
                identities.push(previous.identity());
            }
        }
        if !identities
            .iter()
            .any(|identity| identity.commitment() == step.candidate().identity().commitment())
        {
            identities.push(step.candidate().identity());
        }
        verify_generic_completion(
            &endpoint,
            step.candidate(),
            &identities,
            &hash,
            &inclusion,
            &conditions,
            &cancelled,
        )
    })
    .await
    .map_err(|_| ObservationFailure::Unknown(IpcError::internal()))?
    .map_err(ObservationFailure::from)
}
pub(super) async fn wait_and_verify(
    context: &Context,
    wallet: &WalletControl,
    cancel: &TransferCancellation,
    core: &AppCore,
    step: Arc<PreparedDeclaredContractStep>,
    mut record: StoredEvmTransaction,
) -> Result<CompletedStep, IpcError> {
    let started = std::time::Instant::now();
    loop {
        if started.elapsed() >= std::time::Duration::from_mins(2) {
            return Err(uncertain(
                &step,
                &record.transaction_hash,
                "Receipt wait ended; submission is known, declared outcomes remain unverified",
            ));
        }
        context
            .check(wallet, cancel)
            .map_err(|error| uncertain(&step, &record.transaction_hash, &error.message))?;
        let network = context.network;
        let endpoint = Arc::clone(&context.endpoint);
        let sender = record.sender.clone();
        let nonce = record.nonce.clone();
        let hash = TransactionHash::new(&record.transaction_hash).map_err(error)?;
        let observation = tokio::task::spawn_blocking(move || {
            monitor_transaction(network, &endpoint, &hash, &sender, &nonce)
        })
        .await
        .map_err(|_| {
            uncertain(
                &step,
                &record.transaction_hash,
                "Receipt worker interrupted",
            )
        })?
        .map_err(|_| {
            uncertain(
                &step,
                &record.transaction_hash,
                "Receipt observation unavailable",
            )
        })?;
        context
            .check(wallet, cancel)
            .map_err(|error| uncertain(&step, &record.transaction_hash, &error.message))?;
        if observation.lifecycle() == TransactionLifecycle::Pending {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            continue;
        }
        let (stored, state) = terminal_status(observation.lifecycle())?;
        record.state = stored;
        record.block_number = observation.block_number().map(str::to_owned);
        record.block_hash = observation.block_hash().map(str::to_owned);
        record.updated_at = crate::error_mapping::current_timestamp()?;
        core.save_evm_transaction(&record)
            .map_err(crate::error_mapping::app_core_error)?;
        if !step
            .record_execution(
                ContractStepState::Submitted,
                state,
                Some(&record.transaction_hash),
            )
            .map_err(crate::error_mapping::app_core_error)?
        {
            return Err(IpcError::invalid_request(
                "Workflow state changed while observing receipt; do not retry",
            ));
        }
        if state != ContractStepState::ConfirmedUnverified {
            return Err(IpcError::invalid_request(
                "The included transaction reverted or was replaced; earlier effects may remain. No automatic retry.",
            ));
        }
        let proof = match prove(context, Arc::clone(&step), record.transaction_hash.clone()).await {
            Ok(proof) => proof,
            Err(ObservationFailure::Unsatisfied) => {
                return Err(IpcError::invalid_request(serde_json::json!({"transactionHash":record.transaction_hash,"execution":"confirmed","completionConditions":"unsatisfied at proof-checked observation","goalVerified":false,"retryAutomatically":false}).to_string()));
            }
            Err(ObservationFailure::Unknown(error)) => {
                let _ = step.record_execution(
                    ContractStepState::ConfirmedUnverified,
                    ContractStepState::Uncertain,
                    None,
                );
                return Err(error);
            }
        };
        context.check(wallet, cancel)?;
        context.record_conditions(proof.conditions())?;
        let completion = step
            .apply_completion(proof)
            .map_err(crate::error_mapping::app_core_error)?
            .ok_or_else(|| {
                IpcError::invalid_request("Completion evidence was invalidated before persistence")
            })?;
        return Ok(CompletedStep {
            step,
            completion: Arc::new(completion),
        });
    }
}
fn uncertain(step: &PreparedDeclaredContractStep, hash: &str, message: &str) -> IpcError {
    let _ = step.record_execution(
        ContractStepState::Submitted,
        ContractStepState::Uncertain,
        None,
    );
    IpcError::invalid_request(format!(
        "{message}; known transaction {hash}; no automatic retry or cleanup"
    ))
}

fn terminal_status(
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
