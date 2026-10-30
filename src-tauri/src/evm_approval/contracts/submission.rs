//! The actual transport worker retains the nonce and workflow owners.
use super::preparation::Context;
use crate::{
    evm_approval::registry::unix_now,
    wallet_lifecycle::{TransferCancellation, WalletControl, WalletSubmission},
};
use app_core::{
    ContractStepState, EvmNonceReservation, PreparedContractWorkflowStep, StoredEvmTransaction,
};
use chain_evm::{
    broadcast_signed_transaction_with_admission,
    contract::{ValidatedContractExecution, VerifiedReceiptPredicates},
};
use ipc_types::IpcError;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub(super) struct ReadySubmission {
    pub(super) record: StoredEvmTransaction,
    pub(super) raw: zeroize::Zeroizing<String>,
    pub(super) nonce: EvmNonceReservation,
    pub(super) submission: WalletSubmission,
    pub(super) validated: ValidatedContractExecution,
    pub(super) proofs: Vec<VerifiedReceiptPredicates>,
    pub(super) expiry: u64,
}
pub(super) async fn broadcast(
    context: Arc<Context>,
    wallet: WalletControl,
    cancel: TransferCancellation,
    step: Arc<PreparedContractWorkflowStep>,
    ready: ReadySubmission,
) -> Result<StoredEvmTransaction, IpcError> {
    let record = ready.record.clone();
    let worker_step = Arc::clone(&step);
    let admission = Arc::new(AtomicBool::new(false));
    let admitted = Arc::clone(&admission);
    let entered = Arc::new(AtomicBool::new(false));
    let returned = Arc::new(AtomicBool::new(false));
    let worker_entered = Arc::clone(&entered);
    let worker_returned = Arc::clone(&returned);
    let outcome = tokio::task::spawn_blocking(move || {
        let ReadySubmission {
            record,
            raw,
            nonce,
            submission,
            validated,
            proofs,
            expiry,
        } = ready;
        let mut proofs = Some(proofs);
        broadcast_signed_transaction_with_admission(context.network, &context.endpoint, raw, || {
            worker_entered.store(true, Ordering::Release);
            let ready = context.check(&wallet, &cancel).is_ok()
                && unix_now().is_ok_and(|now| now < expiry)
                && validated.is_fresh()
                && validated.transaction_hash() == record.transaction_hash
                && validated.approved_candidate_digest() == record.candidate_digest;
            let did_admit = super::admission::run(
                ready,
                || nonce.reserve_transaction(&record).is_ok(),
                || {
                    worker_step
                        .mark_dispatched(
                            &record.transaction_hash,
                            &nonce,
                            proofs.take().unwrap_or_default(),
                        )
                        .unwrap_or(false)
                },
                || validated.is_fresh() && cancel.admit(|| submission.begin()).is_ok(),
                |marked| {
                    if marked {
                        let _ = worker_step.discard_known_unsent(&record.transaction_hash, &nonce);
                    } else {
                        let _ = nonce.discard_known_unsent(&record.transaction_hash);
                    }
                },
            );
            admitted.store(did_admit, Ordering::Release);
            worker_returned.store(true, Ordering::Release);
            did_admit
        })
    })
    .await;
    let possible = super::admission::may_have_sent(
        admission.load(Ordering::Acquire),
        entered.load(Ordering::Acquire),
        returned.load(Ordering::Acquire),
    );
    match outcome {
        Ok(Ok(_)) if possible => {
            if !step
                .record_execution(
                    ContractStepState::Dispatching,
                    ContractStepState::Submitted,
                    Some(&record.transaction_hash),
                )
                .map_err(crate::error_mapping::app_core_error)?
            {
                return Err(IpcError::invalid_request(
                    "Submission acknowledged but workflow state changed; reconcile before further writes",
                ));
            }
            Ok(record)
        }
        _ if possible => {
            let _ = step.record_execution(
                ContractStepState::Dispatching,
                ContractStepState::Uncertain,
                Some(&record.transaction_hash),
            );
            Err(IpcError::invalid_request(format!(
                "Submission uncertain for {}; do not retry. Prior allowance may remain.",
                record.transaction_hash
            )))
        }
        _ => {
            let _ = step.stop_before_send(ContractStepState::Failed);
            Err(IpcError::invalid_request(
                "The contract submission was stopped before transport admission",
            ))
        }
    }
}
