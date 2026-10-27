//! Owned actual request cancellation and deadline, independent of UI polling.
use ipc_types::IpcError;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
#[derive(Clone)]
pub(crate) struct TransferCancellation {
    requested: Arc<AtomicBool>,
    interrupted: Arc<AtomicBool>,
    deadline: Instant,
}
impl TransferCancellation {
    pub(crate) fn new(requested: Arc<AtomicBool>) -> Self {
        Self {
            requested,
            interrupted: Arc::new(AtomicBool::new(false)),
            deadline: Instant::now() + Duration::from_mins(2),
        }
    }
    pub(crate) fn is_cancelled(&self) -> bool {
        self.requested.load(Ordering::Acquire)
            || self.interrupted.load(Ordering::Acquire)
            || Instant::now() >= self.deadline
    }
    pub(crate) fn check(&self) -> Result<(), IpcError> {
        if self.is_cancelled() {
            Err(super::super::cancelled_error())
        } else {
            Ok(())
        }
    }
    pub(super) fn interrupt(&self) {
        self.interrupted.store(true, Ordering::Release);
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
        expired.deadline = Instant::now();
        assert!(expired.check().is_err());
    }
}
