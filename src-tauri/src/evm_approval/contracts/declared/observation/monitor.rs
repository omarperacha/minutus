//! Read-only polling has one monotonic budget and never re-enters submission.
use super::{
    super::{
        context::{Context, error},
        lifecycle::{MAX_MONITOR_ATTEMPTS, ObservationStop, StepLifecycle},
    },
    state::stopped,
};
use crate::wallet_lifecycle::{TransferCancellation, WalletControl};
use app_core::{PreparedDeclaredContractStep, StoredEvmTransaction};
use chain_evm::{
    MonitorCause, TransactionHash, TransactionLifecycle, TransactionObservation,
    monitor_transaction_bounded,
};
use ipc_types::IpcError;
use polling::{ReadPolling, ReadResult};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

mod polling;

const RECEIPT_WAIT: Duration = Duration::from_mins(2);
const POLL_DELAY: Duration = Duration::from_secs(1);

pub(super) async fn wait(
    context: &Context,
    wallet: &WalletControl,
    cancel: &TransferCancellation,
    step: &PreparedDeclaredContractStep,
    record: &StoredEvmTransaction,
    progress: &mut StepLifecycle,
) -> Result<TransactionObservation, IpcError> {
    let started = Instant::now();
    progress.receipt_wait_started();
    let hash = TransactionHash::new(&record.transaction_hash).map_err(error)?;
    polling::until_terminal(&mut Session {
        context,
        wallet,
        cancel,
        step,
        record,
        progress,
        hash: &hash,
        started,
    })
    .await
}

struct Session<'a> {
    context: &'a Context,
    wallet: &'a WalletControl,
    cancel: &'a TransferCancellation,
    step: &'a PreparedDeclaredContractStep,
    record: &'a StoredEvmTransaction,
    progress: &'a mut StepLifecycle,
    hash: &'a TransactionHash,
    started: Instant,
}

impl ReadPolling for Session<'_> {
    type Observation = TransactionObservation;
    type Error = IpcError;

    fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }
    fn check(&mut self) -> Result<(), IpcError> {
        check(
            self.context,
            self.wallet,
            self.cancel,
            self.step,
            self.progress,
            self.hash.as_str(),
        )
    }
    fn attempted(&mut self) -> Result<(), IpcError> {
        self.progress.receipt_attempted()
    }
    async fn read(&mut self) -> Result<ReadResult<TransactionObservation>, IpcError> {
        match poll(self.context, self.record, self.hash, self.started).await {
            Err(_) => Err(stopped(
                self.step,
                self.progress,
                self.hash.as_str(),
                ObservationStop::WorkerInterrupted,
                "Receipt observation worker was interrupted",
            )),
            Ok(Err(failure)) => {
                self.progress.receipt_monitor_failed(failure);
                if !failure.retryable_read() {
                    let reason = match failure.cause() {
                        MonitorCause::BudgetElapsed => ObservationStop::BudgetElapsed,
                        MonitorCause::Cancelled => ObservationStop::ContextUnavailable,
                        _ => ObservationStop::MonitorRejected,
                    };
                    return Err(stopped(
                        self.step,
                        self.progress,
                        self.hash.as_str(),
                        reason,
                        "Receipt observation could not be verified",
                    ));
                }
                Ok(ReadResult::RetryEligible)
            }
            Ok(Ok(observation)) => Ok(ReadResult::Observed(observation)),
        }
    }
    fn reported(&mut self, value: &TransactionObservation) -> TransactionLifecycle {
        self.progress.receipt_reported(value.lifecycle());
        value.lifecycle()
    }
    async fn pause(&mut self, delay: Duration) {
        tokio::time::sleep(delay).await;
    }
    fn stop(&mut self, reason: ObservationStop) -> IpcError {
        let message = match reason {
            ObservationStop::AttemptsExhausted => {
                "The bounded receipt monitoring attempt limit was reached"
            }
            _ => "Receipt monitoring budget ended; completion remains unverified",
        };
        stopped(
            self.step,
            self.progress,
            self.hash.as_str(),
            reason,
            message,
        )
    }
}

async fn poll(
    context: &Context,
    record: &StoredEvmTransaction,
    hash: &TransactionHash,
    started: Instant,
) -> Result<Result<TransactionObservation, chain_evm::MonitorFailure>, tokio::task::JoinError> {
    let endpoint = Arc::clone(&context.endpoint);
    let network = context.network;
    let sender = record.sender.clone();
    let nonce = record.nonce.clone();
    let hash = hash.clone();
    let cancelled = Arc::clone(&context.cancelled);
    tokio::task::spawn_blocking(move || {
        monitor_transaction_bounded(
            network,
            &endpoint,
            &hash,
            &sender,
            &nonce,
            remaining(started.elapsed()).unwrap_or_default(),
            &cancelled,
        )
    })
    .await
}

fn check(
    context: &Context,
    wallet: &WalletControl,
    cancel: &TransferCancellation,
    step: &PreparedDeclaredContractStep,
    progress: &mut StepLifecycle,
    hash: &str,
) -> Result<(), IpcError> {
    context.check(wallet, cancel).map_err(|error| {
        stopped(
            step,
            progress,
            hash,
            ObservationStop::ContextUnavailable,
            &error.message,
        )
    })
}
fn remaining(elapsed: Duration) -> Option<Duration> {
    RECEIPT_WAIT
        .checked_sub(elapsed)
        .filter(|remaining| !remaining.is_zero())
}

#[cfg(test)]
mod tests;
