//! Wallet-owned transfer lifetime checks, including native unlock cancellation.
use super::WalletControl;
mod cancellation;
pub(crate) use cancellation::{TransferCancellation, TransferCancellationReason};
use ipc_types::{IpcError, WalletAvailability};
use std::{
    sync::Mutex,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
impl WalletControl {
    fn cancel_transfer_review(&self, app: &tauri::AppHandle, expected_epoch: u64) {
        #[cfg(target_os = "macos")]
        let Some(abort) = super::native::modal::ModalAbort::capture_if(|| {
            self.lock_if_transfer_epoch(expected_epoch)
        }) else {
            super::native::modal::ModalAbort::queue_cancelled(app);
            return;
        };
        #[cfg(not(target_os = "macos"))]
        if !self.lock_if_transfer_epoch(expected_epoch) {
            return;
        }
        #[cfg(target_os = "macos")]
        abort.queue(app);
        #[cfg(not(target_os = "macos"))]
        let _ = app;
    }
    pub(crate) fn transfer_epoch(&self) -> Result<u64, IpcError> {
        let epoch = self.epoch.load(Ordering::Acquire);
        self.check_transfer_epoch(epoch)?;
        Ok(epoch)
    }
    pub(crate) fn transfer_epoch_is_current(&self, epoch: u64) -> bool {
        !self.revoked.load(Ordering::Acquire) && self.epoch.load(Ordering::Acquire) == epoch
    }
    pub(crate) fn check_transfer_epoch(&self, epoch: u64) -> Result<(), IpcError> {
        if self.public_state()?.availability != WalletAvailability::Ready
            || self.revoked.load(Ordering::Acquire)
            || self.epoch.load(Ordering::Acquire) != epoch
        {
            return Err(super::cancelled_error());
        }
        Ok(())
    }

    pub(crate) fn transfer_work<T>(
        &self,
        app: &tauri::AppHandle,
        cancelled: &std::sync::Arc<AtomicBool>,
        epoch: Option<u64>,
        operation: impl FnOnce(&TransferCancellation) -> Result<T, IpcError>,
    ) -> Result<T, IpcError> {
        let control = TransferCancellation::new(std::sync::Arc::clone(cancelled));
        self.run_transfer_work(app, &control, epoch, operation)
    }
    /// Contract work uses bounded individual phases and exact approval expiry;
    /// user waiting never consumes an unrelated whole-workflow signing deadline.
    pub(crate) fn contract_workflow_work<T>(
        &self,
        app: &tauri::AppHandle,
        cancelled: &std::sync::Arc<AtomicBool>,
        operation: impl FnOnce(&TransferCancellation) -> Result<T, IpcError>,
    ) -> Result<T, IpcError> {
        let control = TransferCancellation::contract_workflow(std::sync::Arc::clone(cancelled));
        self.run_transfer_work(app, &control, None, operation)
    }
    fn run_transfer_work<T>(
        &self,
        app: &tauri::AppHandle,
        control: &TransferCancellation,
        epoch: Option<u64>,
        operation: impl FnOnce(&TransferCancellation) -> Result<T, IpcError>,
    ) -> Result<T, IpcError> {
        control.check()?;
        let active = Mutex::new(true);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                loop {
                    // Retirement and actual revocation share this fence. A
                    // watcher paused before cancellation cannot lock a later
                    // request after the completed operation has retired.
                    let Some(owned) = active_owner(&active) else {
                        break;
                    };
                    let bound = control.wallet_epoch();
                    let expected = bound.as_ref().ok().copied().flatten().or(epoch);
                    let epoch_changed = bound.is_err()
                        || expected.is_some_and(|epoch| !self.transfer_epoch_is_current(epoch));
                    if control.is_cancelled() || epoch_changed {
                        if epoch_changed {
                            control.interrupt_with_reason(
                                TransferCancellationReason::WalletEpochChanged,
                            );
                        } else {
                            control.interrupt();
                        }
                        self.cancel_transfer_review(
                            app,
                            expected.unwrap_or_else(|| self.epoch.load(Ordering::Acquire)),
                        );
                        break;
                    }
                    drop(owned);
                    std::thread::sleep(Duration::from_millis(25));
                }
            });
            let _finish = Finish(&active);
            operation(control)
        })
    }
}
struct Finish<'a>(&'a Mutex<bool>);
fn active_owner(active: &Mutex<bool>) -> Option<std::sync::MutexGuard<'_, bool>> {
    active.lock().ok().filter(|owned| **owned)
}
impl Drop for Finish<'_> {
    fn drop(&mut self) {
        if let Ok(mut active) = self.0.lock() {
            *active = false;
        }
    }
}

#[cfg(test)]
mod lifetime_tests {
    use super::*;
    #[test]
    fn watcher_paused_before_actual_mutation_cannot_revoke_after_retirement()
    -> Result<(), Box<dyn std::error::Error>> {
        let active = std::sync::Arc::new(Mutex::new(true));
        let retired = Finish(&active);
        let (observed, observed_rx) = std::sync::mpsc::channel();
        let (resume, resume_rx) = std::sync::mpsc::channel();
        let watcher = std::sync::Arc::clone(&active);
        let revoked = std::sync::Arc::new(AtomicBool::new(false));
        let mutation = std::sync::Arc::clone(&revoked);
        let thread = std::thread::spawn(move || {
            let _ = observed.send(());
            if resume_rx.recv().is_ok() && active_owner(&watcher).is_some() {
                mutation.store(true, Ordering::Release);
            }
        });
        observed_rx.recv_timeout(Duration::from_secs(1))?;
        drop(retired);
        resume.send(())?;
        thread.join().map_err(|_| "watcher panicked")?;
        assert!(!revoked.load(Ordering::Acquire));
        Ok(())
    }
    #[test]
    fn old_epoch_cannot_lock_a_newly_admitted_wallet_job() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = tempfile::tempdir()?;
        let control = WalletControl::initialize(directory.path().join("synthetic"), None)?;
        let old = control.epoch.load(Ordering::Acquire);
        control.lock();
        let new_job = control.begin().map_err(|error| error.message)?;
        assert!(!control.lock_if_transfer_epoch(old));
        assert!(new_job.check().is_ok());
        assert!(!control.revoked.load(Ordering::Acquire));
        Ok(())
    }
}
