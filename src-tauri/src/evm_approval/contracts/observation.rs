use super::preparation::{Context, chain_error};
use crate::wallet_lifecycle::{TransferCancellation, WalletControl};
use app_core::{
    AppCore, ContractStepState, PreparedContractWorkflowStep, StoredEvmTransaction,
    StoredEvmTransactionState,
};
use chain_evm::{
    TransactionHash, TransactionLifecycle,
    contract::{
        VerifiedReceiptPredicates, acquire_contract_transaction_proof,
        verify_contract_receipt_predicates,
    },
    monitor_transaction,
};
use ipc_types::IpcError;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

pub(super) struct CompletedStep {
    pub(super) step: Arc<PreparedContractWorkflowStep>,
    pub(super) hash: String,
    last_observation: Mutex<super::reporting::ObservedFacts>,
}
pub(super) async fn prerequisites(
    context: &Context,
    completed: &[CompletedStep],
) -> Result<Vec<VerifiedReceiptPredicates>, IpcError> {
    let mut proofs = Vec::with_capacity(completed.len());
    for prior in completed {
        let proof = prove(context, Arc::clone(&prior.step), prior.hash.clone()).await?;
        let mut snapshot = prior
            .last_observation
            .lock()
            .map_err(|_| IpcError::internal())?;
        *snapshot = super::reporting::capture(proof_facts(&proof))?;
        drop(snapshot);
        proofs.push(proof);
    }
    Ok(proofs)
}
pub(super) async fn prove(
    context: &Context,
    step: Arc<PreparedContractWorkflowStep>,
    hash: String,
) -> Result<VerifiedReceiptPredicates, IpcError> {
    let network = context.network;
    let endpoint = context.endpoint.clone();
    let cancelled = Arc::clone(&context.cancelled);
    let predicates = step
        .predicates()
        .map_err(crate::error_mapping::app_core_error)?;
    tokio::task::spawn_blocking(move || {
        let inclusion = acquire_contract_transaction_proof(network, &endpoint, &hash, &cancelled)?;
        verify_contract_receipt_predicates(
            &endpoint,
            step.candidate(),
            &hash,
            &inclusion,
            &predicates,
            &cancelled,
        )
    })
    .await
    .map_err(|_| IpcError::internal())?
    .map_err(chain_error)
}
pub(super) async fn wait_and_verify(
    context: &Context,
    wallet: &WalletControl,
    cancel: &TransferCancellation,
    core: &AppCore,
    step: Arc<PreparedContractWorkflowStep>,
    mut record: StoredEvmTransaction,
) -> Result<CompletedStep, IpcError> {
    let started = std::time::Instant::now();
    loop {
        if started.elapsed() >= std::time::Duration::from_mins(2) {
            let _ = step.record_execution(
                ContractStepState::Submitted,
                ContractStepState::Uncertain,
                None,
            );
            return Err(IpcError::invalid_request(format!(
                "Receipt wait ended for {}; submission remains known, task predicates are unverified. Do not retry automatically.",
                record.transaction_hash
            )));
        }
        context
            .check(wallet, cancel)
            .map_err(|error| uncertain(&step, &record.transaction_hash, &error.message))?;
        let network = context.network;
        let endpoint = context.endpoint.clone();
        let sender = record.sender.clone();
        let nonce = record.nonce.clone();
        let hash = TransactionHash::new(&record.transaction_hash).map_err(chain_error)?;
        let observation = tokio::task::spawn_blocking(move || {
            monitor_transaction(network, &endpoint, &hash, &sender, &nonce)
        })
        .await
        .map_err(|_| IpcError::internal())?
        .map_err(chain_error)?;
        context.check(wallet, cancel)?;
        if observation.lifecycle() == TransactionLifecycle::Pending {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            continue;
        }
        let (stored, state) = match observation.lifecycle() {
            TransactionLifecycle::Confirmed => (
                StoredEvmTransactionState::Confirmed,
                ContractStepState::ConfirmedUnverified,
            ),
            TransactionLifecycle::Failed => {
                (StoredEvmTransactionState::Failed, ContractStepState::Failed)
            }
            TransactionLifecycle::DroppedOrReplaced => (
                StoredEvmTransactionState::DroppedOrReplaced,
                ContractStepState::Uncertain,
            ),
            TransactionLifecycle::Pending => return Err(IpcError::internal()),
        };
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
                "Workflow state changed while observing receipt",
            ));
        }
        if state != ContractStepState::ConfirmedUnverified {
            return Err(IpcError::invalid_request(
                "The contract transaction failed or was replaced; later steps are stopped and prior allowance may remain",
            ));
        }
        let proof = match prove(context, Arc::clone(&step), record.transaction_hash.clone()).await {
            Ok(proof) => proof,
            Err(error) => {
                let _ = step.record_execution(
                    ContractStepState::ConfirmedUnverified,
                    ContractStepState::Uncertain,
                    None,
                );
                return Err(error);
            }
        };
        context.check(wallet, cancel)?;
        let last_observation = Mutex::new(super::reporting::capture(proof_facts(&proof))?);
        if !step
            .apply_verified_postcondition(proof)
            .map_err(crate::error_mapping::app_core_error)?
        {
            return Err(IpcError::invalid_request(
                "Receipt predicate was invalidated before it could be recorded",
            ));
        }
        return Ok(CompletedStep {
            step,
            hash: record.transaction_hash,
            last_observation,
        });
    }
}

fn snapshots(
    completed: &[CompletedStep],
) -> Result<Vec<super::reporting::ObservedFacts>, IpcError> {
    completed
        .iter()
        .map(|step| {
            let mut value = step
                .last_observation
                .lock()
                .map_err(|_| IpcError::internal())?
                .clone();
            value
                .facts
                .as_object_mut()
                .ok_or_else(IpcError::internal)?
                .insert("stepIndex".into(), json!(step.step.index()));
            Ok(value)
        })
        .collect()
}
pub(super) fn complete_facts(
    completed: &[CompletedStep],
) -> Result<super::reporting::ProofReport, IpcError> {
    super::reporting::complete(snapshots(completed)?)
}
pub(super) fn residual_facts(completed: &[CompletedStep]) -> Value {
    match snapshots(completed) {
        Ok(facts) => super::reporting::partial(facts),
        Err(_) => {
            json!({"currentResidualAllowance":"unknown; proof observation context unavailable","lastVerifiedPredicates":[],"lastObservedRemainingAllowances":[],"automaticCleanup":false,"retryAutomatically":false})
        }
    }
}
fn proof_facts(proof: &VerifiedReceiptPredicates) -> Value {
    let predicates=proof.observations().iter().map(|observed|match observed.predicate(){
        chain_evm::contract::ContractPredicate::Allowance {owner,spender,..}=>json!({"kind":"allowance","token":proof.target(),"owner":owner,"spender":spender,"value":observed.value()}),
        chain_evm::contract::ContractPredicate::TokenBalance {account,..}=>json!({"kind":"tokenBalance","token":proof.target(),"account":account,"value":observed.value()}),
    }).collect::<Vec<_>>();
    json!({"transactionHash":proof.transaction_hash(),"network":proof.network().descriptor().caip2_id(),"receiptBlockNumber":proof.receipt_block_number().to_string(),"receiptBlockHash":proof.receipt_block_hash(),"stateBlockNumber":proof.state_block_number().to_string(),"stateBlockHash":proof.state_block_hash(),"receiptCommitment":proof.receipt_commitment(),"proofCommitment":proof.proof_commitment(),"profileId":proof.profile_id(),"profileCommitment":proof.profile_commitment(),"predicates":predicates})
}

fn uncertain(step: &PreparedContractWorkflowStep, hash: &str, message: &str) -> IpcError {
    let _ = step.record_execution(
        ContractStepState::Submitted,
        ContractStepState::Uncertain,
        None,
    );
    IpcError::invalid_request(format!(
        "{message}; known submitted transaction {hash}; task predicates remain unverified. No automatic retry."
    ))
}
