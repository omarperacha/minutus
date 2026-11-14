//! Owned actual request cancellation and deadline, independent of UI polling.
use ipc_types::IpcError;
use serde::Serialize;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};
#[derive(Clone)]
pub(crate) struct TransferCancellation {
    requested: Arc<AtomicBool>,
    worker_cancelled: Arc<AtomicBool>,
    interrupted: Arc<AtomicU8>,
    wallet_epoch: Arc<Mutex<Option<u64>>>,
    deadline: Option<Instant>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[repr(u8)]
pub(crate) enum TransferCancellationReason {
    RequestCancelled = 1,
    DeadlineElapsed = 2,
    Interrupted = 3,
    WalletEpochChanged = 4,
}
impl TransferCancellation {
    pub(crate) fn new(requested: Arc<AtomicBool>) -> Self {
        Self {
            requested,
            worker_cancelled: Arc::new(AtomicBool::new(false)),
            interrupted: Arc::new(AtomicU8::new(0)),
            wallet_epoch: Arc::new(Mutex::new(None)),
            deadline: Some(Instant::now() + Duration::from_mins(2)),
        }
    }
    // Only native contract workflow composition may select phase-owned limits.
    // Each source/execution and exact approval keeps its own unchanged budget.
    pub(super) fn contract_workflow(requested: Arc<AtomicBool>) -> Self {
        Self {
            requested,
            worker_cancelled: Arc::new(AtomicBool::new(false)),
            interrupted: Arc::new(AtomicU8::new(0)),
            wallet_epoch: Arc::new(Mutex::new(None)),
            deadline: None,
        }
    }
    pub(crate) fn worker_cancellation(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.worker_cancelled)
    }
    /// Pins initial successful native readiness once. No later unlock or account
    /// selection may rearm this same workflow attempt.
    pub(crate) fn bind_wallet_epoch(&self, epoch: u64) -> Result<(), IpcError> {
        self.check()?;
        let mut bound = self
            .wallet_epoch
            .lock()
            .map_err(|_| super::super::cancelled_error())?;
        if bound.is_some_and(|existing| existing != epoch) {
            return Err(super::super::cancelled_error());
        }
        *bound = Some(epoch);
        self.check()
    }
    pub(super) fn wallet_epoch(&self) -> Result<Option<u64>, IpcError> {
        self.wallet_epoch
            .lock()
            .map(|value| *value)
            .map_err(|_| super::super::cancelled_error())
    }
    pub(crate) fn is_cancelled(&self) -> bool {
        let cancelled = self.reason().is_some();
        if cancelled {
            self.worker_cancelled.store(true, Ordering::Release);
        }
        cancelled
    }
    pub(crate) fn reason(&self) -> Option<TransferCancellationReason> {
        self.reason_at(Instant::now())
    }
    fn reason_at(&self, now: Instant) -> Option<TransferCancellationReason> {
        let interrupted = match self.interrupted.load(Ordering::Acquire) {
            1 => Some(TransferCancellationReason::RequestCancelled),
            2 => Some(TransferCancellationReason::DeadlineElapsed),
            3 => Some(TransferCancellationReason::Interrupted),
            4..=u8::MAX => Some(TransferCancellationReason::WalletEpochChanged),
            0 => None,
        };
        if interrupted.is_some() {
            interrupted
        } else if self.requested.load(Ordering::Acquire) {
            Some(TransferCancellationReason::RequestCancelled)
        } else if self.deadline.is_some_and(|deadline| now >= deadline) {
            Some(TransferCancellationReason::DeadlineElapsed)
        } else {
            None
        }
    }
    pub(crate) fn check(&self) -> Result<(), IpcError> {
        if self.is_cancelled() {
            Err(super::super::cancelled_error())
        } else {
            Ok(())
        }
    }
    pub(super) fn interrupt(&self) {
        self.interrupt_with_reason(
            self.reason()
                .unwrap_or(TransferCancellationReason::Interrupted),
        );
    }
    pub(super) fn interrupt_with_reason(&self, reason: TransferCancellationReason) {
        let _ =
            self.interrupted
                .compare_exchange(0, reason as u8, Ordering::AcqRel, Ordering::Acquire);
        self.worker_cancelled.store(true, Ordering::Release);
    }
    /// The final synchronous request check is the transport admission point.
    pub(crate) fn admit(
        &self,
        owner_check: impl FnOnce() -> Result<(), IpcError>,
    ) -> Result<(), IpcError> {
        self.check()?;
        owner_check()?;
        self.check()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_before_admission_stops_send_without_a_watcher() {
        let requested = Arc::new(AtomicBool::new(false));
        let cancellation = TransferCancellation::new(Arc::clone(&requested));
        let mut sends = 0;
        requested.store(true, Ordering::Release);
        if cancellation.admit(|| Ok(())).is_ok() {
            sends += 1;
        }
        assert_eq!(sends, 0);
    }
    #[test]
    fn cancellation_during_owner_io_is_rechecked_before_send() {
        let requested = Arc::new(AtomicBool::new(false));
        let cancellation = TransferCancellation::new(Arc::clone(&requested));
        let admitted = cancellation.admit(|| {
            requested.store(true, Ordering::Release);
            Ok(())
        });
        assert!(admitted.is_err());
    }
    #[test]
    fn an_owned_clone_retains_the_actual_request_and_deadline() {
        let requested = Arc::new(AtomicBool::new(false));
        let control = TransferCancellation::new(Arc::clone(&requested));
        let worker = control.clone();
        drop(control);
        requested.store(true, Ordering::Release);
        assert!(worker.check().is_err());
        let mut expired = TransferCancellation::new(Arc::new(AtomicBool::new(false)));
        expired.deadline = Some(Instant::now());
        assert!(expired.check().is_err());
    }
    #[test]
    fn workflow_human_wait_does_not_consume_an_ordinary_transfer_deadline() {
        let request = Arc::new(AtomicBool::new(false));
        let normal = TransferCancellation::new(Arc::clone(&request));
        let workflow = TransferCancellation::contract_workflow(Arc::clone(&request));
        let later = Instant::now() + Duration::from_secs(121);
        assert_eq!(
            normal.reason_at(later),
            Some(TransferCancellationReason::DeadlineElapsed)
        );
        assert_eq!(workflow.reason_at(later), None);
        request.store(true, Ordering::Release);
        assert_eq!(
            workflow.reason_at(later),
            Some(TransferCancellationReason::RequestCancelled)
        );
        let workflow = TransferCancellation::contract_workflow(Arc::new(AtomicBool::new(false)));
        workflow.interrupt();
        assert_eq!(
            workflow.reason_at(later),
            Some(TransferCancellationReason::Interrupted)
        );
    }
    #[test]
    fn workflow_epoch_is_single_assignment_and_internal_stop_reaches_workers()
    -> Result<(), IpcError> {
        let workflow = TransferCancellation::contract_workflow(Arc::new(AtomicBool::new(false)));
        workflow.bind_wallet_epoch(7)?;
        workflow.bind_wallet_epoch(7)?;
        assert!(workflow.bind_wallet_epoch(8).is_err());
        let worker = workflow.worker_cancellation();
        assert!(!worker.load(Ordering::Acquire));
        workflow.interrupt_with_reason(TransferCancellationReason::WalletEpochChanged);
        assert!(worker.load(Ordering::Acquire));
        assert_eq!(
            workflow.reason(),
            Some(TransferCancellationReason::WalletEpochChanged)
        );
        assert!(workflow.bind_wallet_epoch(7).is_err());
        Ok(())
    }
}
