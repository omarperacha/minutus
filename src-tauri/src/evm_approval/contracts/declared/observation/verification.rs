//! Only applied proof-backed completions can advance the immutable workflow.
use super::{
    super::{
        context::Context,
        lifecycle::{ObservationStop, StepLifecycle},
    },
    CompletedStep, ObservationFailure, monitor,
    proof::{self, ProofFailure},
    state::{stopped, terminal_status},
};
use crate::wallet_lifecycle::{TransferCancellation, WalletControl};
use app_core::{AppCore, ContractStepState, PreparedDeclaredContractStep, StoredEvmTransaction};
use ipc_types::IpcError;
use std::sync::Arc;

pub(in crate::evm_approval::contracts::declared) async fn wait_and_verify(
    context: &Context,
    wallet: &WalletControl,
    cancel: &TransferCancellation,
    core: &AppCore,
    step: Arc<PreparedDeclaredContractStep>,
    mut record: StoredEvmTransaction,
    progress: &mut StepLifecycle,
) -> Result<CompletedStep, IpcError> {
    let observation = monitor::wait(context, wallet, cancel, &step, &record, progress).await?;
    let state = persist_receipt(core, &step, &mut record, &observation, progress)?;
    if state != ContractStepState::ConfirmedUnverified {
        return Err(IpcError::invalid_request(
            "RPC reported a reverted transaction or an unavailable/replaced transaction; later writes remain stopped. Canonical outcome is not established by this report alone. No automatic retry.",
        ));
    }
    let proof = verify_completion(context, &step, &record.transaction_hash, progress).await?;
    context.check(wallet, cancel).map_err(|failure| {
        stopped(
            &step,
            progress,
            &record.transaction_hash,
            ObservationStop::CompletionUnavailable,
            &failure.message,
        )
    })?;
    context
        .record_conditions(proof.conditions())
        .map_err(|failure| {
            stopped(
                &step,
                progress,
                &record.transaction_hash,
                ObservationStop::PersistenceUnavailable,
                &failure.message,
            )
        })?;
    let completion = step
        .apply_completion(proof)
        .map_err(|failure| {
            stopped(
                &step,
                progress,
                &record.transaction_hash,
                ObservationStop::PersistenceUnavailable,
                &crate::error_mapping::app_core_error(failure).message,
            )
        })?
        .ok_or_else(|| {
            stopped(
                &step,
                progress,
                &record.transaction_hash,
                ObservationStop::PersistenceUnavailable,
                "Completion evidence was invalidated before persistence",
            )
        })?;
    progress.completion_verified();
    Ok(CompletedStep {
        step,
        completion: Arc::new(completion),
    })
}

fn persist_receipt(
    core: &AppCore,
    step: &PreparedDeclaredContractStep,
    record: &mut StoredEvmTransaction,
    observation: &chain_evm::TransactionObservation,
    progress: &mut StepLifecycle,
) -> Result<ContractStepState, IpcError> {
    let (stored, state) = terminal_status(observation.lifecycle())?;
    record.state = stored;
    record.block_number = observation.block_number().map(str::to_owned);
    record.block_hash = observation.block_hash().map(str::to_owned);
    record.updated_at = crate::error_mapping::current_timestamp()?;
    core.save_evm_transaction(record).map_err(|failure| {
        stopped(
            step,
            progress,
            &record.transaction_hash,
            ObservationStop::PersistenceUnavailable,
            &crate::error_mapping::app_core_error(failure).message,
        )
    })?;
    if !step
        .record_execution(
            ContractStepState::Submitted,
            state,
            Some(&record.transaction_hash),
        )
        .map_err(|failure| {
            stopped(
                step,
                progress,
                &record.transaction_hash,
                ObservationStop::PersistenceUnavailable,
                &crate::error_mapping::app_core_error(failure).message,
            )
        })?
    {
        return Err(stopped(
            step,
            progress,
            &record.transaction_hash,
            ObservationStop::PersistenceUnavailable,
            "Workflow state changed while recording the receipt",
        ));
    }
    Ok(state)
}

async fn verify_completion(
    context: &Context,
    step: &Arc<PreparedDeclaredContractStep>,
    hash: &str,
    progress: &mut StepLifecycle,
) -> Result<chain_evm::contract::generic::VerifiedGenericCompletion, IpcError> {
    progress.inclusion_started();
    let proof = match proof::prove(context, Arc::clone(step), hash.to_owned()).await {
        Ok(proof) => {
            progress.inclusion_verified();
            progress.completion_started();
            proof
        }
        Err(ProofFailure::Completion(failure)) => {
            progress.inclusion_verified();
            progress.completion_started();
            if matches!(failure, ObservationFailure::Unsatisfied) {
                progress.completion_unsatisfied();
                return Err(IpcError::invalid_request(serde_json::json!({"transactionHash":hash,"execution":"canonical inclusion proved","completionConditions":"unsatisfied at proof-checked observation","goalVerified":false,"retryAutomatically":false}).to_string()));
            }
            let ObservationFailure::Unknown(error) = failure else {
                return Err(IpcError::internal());
            };
            return Err(stopped(
                step,
                progress,
                hash,
                ObservationStop::CompletionUnavailable,
                &error.message,
            ));
        }
        Err(ProofFailure::Inclusion(failure)) => {
            let message = match failure {
                ObservationFailure::Unknown(error) => error.message,
                ObservationFailure::Unsatisfied => "Canonical inclusion was not established".into(),
            };
            return Err(stopped(
                step,
                progress,
                hash,
                ObservationStop::InclusionUnavailable,
                &message,
            ));
        }
        Err(ProofFailure::Worker) => {
            return Err(stopped(
                step,
                progress,
                hash,
                ObservationStop::InclusionUnavailable,
                "Proof worker interrupted; usable inclusion/completion evidence was not returned",
            ));
        }
    };
    Ok(proof)
}
