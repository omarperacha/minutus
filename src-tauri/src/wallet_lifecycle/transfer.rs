//! Wallet-owned transfer lifetime checks, including native unlock cancellation.
use super::WalletControl;
mod cancellation;
pub(crate) use cancellation::TransferCancellation;
use ipc_types::{IpcError, WalletAvailability};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
impl WalletControl {
    pub(crate) fn cancel_transfer_review(&self, app: &tauri::AppHandle) {
        self.lock();
        #[cfg(target_os = "macos")]
        super::native::abort_prompt(app);
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
        control.check()?;
        let finished = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                while !finished.load(Ordering::Acquire) {
                    if control.is_cancelled()
                        || epoch.is_some_and(|epoch| !self.transfer_epoch_is_current(epoch))
                    {
                        control.interrupt();
                        self.cancel_transfer_review(app);
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
            });
            let _finish = Finish(&finished);
            operation(&control)
        })
    }
}
struct Finish<'a>(&'a AtomicBool);
impl Drop for Finish<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
