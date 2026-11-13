//! Exact native-modal ownership prevents queued aborts from targeting successors.
use ipc_types::IpcError;
use objc2::{MainThreadMarker, rc::Retained};
use objc2_app_kit::{NSApplication, NSWindow};
use std::{
    cell::RefCell,
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct Registry {
    next: u64,
    current: u64,
    revocation: u64,
}
impl Registry {
    fn admit(&mut self, generation: u64) -> Result<u64, IpcError> {
        if !current_request(generation, self.revocation) {
            return Err(super::super::cancelled_error());
        }
        self.next = self.next.checked_add(1).ok_or_else(IpcError::internal)?;
        self.current = self.next;
        Ok(self.current)
    }
    fn revoke(&mut self) -> u64 {
        let expected = self.current;
        self.revocation = self.revocation.saturating_add(1);
        expected
    }
    fn revoke_if(&mut self, revoke: impl FnOnce() -> bool) -> Option<u64> {
        if !revoke() {
            return None;
        }
        Some(self.revoke())
    }
}
static REGISTRY: Mutex<Registry> = Mutex::new(Registry {
    next: 0,
    current: 0,
    revocation: 0,
});
type CancellationCheck = Arc<dyn Fn() -> bool + Send + Sync>;
struct ModalWindow {
    id: u64,
    window: Retained<NSWindow>,
    cancelled: Option<CancellationCheck>,
}
thread_local! { static WINDOWS: RefCell<Vec<ModalWindow>> = const { RefCell::new(Vec::new()) }; }

pub(crate) struct RegisteredModal {
    id: u64,
    _main_thread: MainThreadMarker,
}
pub(crate) struct ModalRequest {
    generation: u64,
    cancelled: Option<CancellationCheck>,
}
impl ModalRequest {
    pub(crate) fn new() -> Self {
        Self {
            generation: REGISTRY
                .lock()
                .map_or(u64::MAX, |registry| registry.revocation),
            cancelled: None,
        }
    }
    pub(super) fn for_job(job: &wallet_core::PasswordVaultJob) -> Self {
        Self::with_cancellation(job.cancellation_check())
    }
    fn with_cancellation(check: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        Self {
            cancelled: Some(Arc::new(check)),
            ..Self::new()
        }
    }
    pub(crate) fn for_transfer(cancel: &crate::wallet_lifecycle::TransferCancellation) -> Self {
        let cancel = cancel.clone();
        Self::with_cancellation(move || cancel.is_cancelled())
    }
    pub(crate) fn check(&self) -> Result<(), IpcError> {
        if REGISTRY
            .lock()
            .is_ok_and(|registry| current_request(self.generation, registry.revocation))
            && !self.cancelled.as_ref().is_some_and(|check| check())
        {
            Ok(())
        } else {
            Err(super::super::cancelled_error())
        }
    }
    pub(crate) fn register(&self, window: Retained<NSWindow>) -> Result<RegisteredModal, IpcError> {
        self.check()?;
        let registration =
            RegisteredModal::register(window, self.generation, self.cancelled.clone())?;
        self.check()?;
        Ok(registration)
    }
}
impl RegisteredModal {
    fn register(
        window: Retained<NSWindow>,
        generation: u64,
        cancelled: Option<CancellationCheck>,
    ) -> Result<Self, IpcError> {
        let main_thread = MainThreadMarker::new().ok_or_else(IpcError::internal)?;
        let mut registry = REGISTRY.lock().map_err(|_| IpcError::internal())?;
        let id = registry.admit(generation)?;
        WINDOWS.with(|windows| {
            windows.borrow_mut().push(ModalWindow {
                id,
                window,
                cancelled,
            });
        });
        Ok(Self {
            id,
            _main_thread: main_thread,
        })
    }
}
impl Drop for RegisteredModal {
    fn drop(&mut self) {
        WINDOWS.with(|windows| {
            let mut windows = windows.borrow_mut();
            windows.retain(|entry| entry.id != self.id);
            if let Ok(mut registry) = REGISTRY.lock() {
                registry.current = windows.last().map_or(0, |entry| entry.id);
            }
        });
    }
}
pub(crate) struct ModalAbort {
    expected: u64,
}
impl ModalAbort {
    pub(in crate::wallet_lifecycle) fn capture_if(revoke: impl FnOnce() -> bool) -> Option<Self> {
        let mut registry = REGISTRY.lock().unwrap_or_else(|poison| {
            let mut registry = poison.into_inner();
            // Broken UI bookkeeping can only deny future presentation; it must
            // never suppress actual wallet/security revocation.
            registry.revocation = u64::MAX;
            registry
        });
        // Snapshot and retire queued requests atomically with conditional wallet
        // revocation. A stale epoch cannot revoke a successor's UI.
        let expected = registry.revoke_if(revoke)?;
        Some(Self { expected })
    }
    pub(crate) fn queue(self, app: &tauri::AppHandle) {
        // Capture before queueing: a successor must never inherit an older abort.
        let expected = self.expected;
        if expected == 0 {
            return;
        }
        let _ = app.run_on_main_thread(move || {
            let Some(main_thread) = MainThreadMarker::new() else {
                return;
            };
            let application = NSApplication::sharedApplication(main_thread);
            let should_abort = WINDOWS.with(|windows| {
                let windows = windows.borrow();
                let Some(ModalWindow { id, window, .. }) = windows.last() else {
                    return false;
                };
                let same_window = application
                    .modalWindow()
                    .is_some_and(|actual| std::ptr::eq(&raw const *actual, &raw const **window));
                matches_target(expected, *id, same_window)
            });
            if should_abort {
                application.abortModal();
            }
        });
    }
    /// No registry revocation: only the displayed dialog's own stopped context
    /// may justify this abort. A live successor is left untouched.
    pub(in crate::wallet_lifecycle) fn queue_cancelled(app: &tauri::AppHandle) {
        let _ = app.run_on_main_thread(|| {
            let Some(main_thread) = MainThreadMarker::new() else {
                return;
            };
            let application = NSApplication::sharedApplication(main_thread);
            let should_abort = WINDOWS.with(|windows| {
                let windows = windows.borrow();
                let Some(ModalWindow {
                    window, cancelled, ..
                }) = windows.last()
                else {
                    return false;
                };
                let same_window = application
                    .modalWindow()
                    .is_some_and(|actual| std::ptr::eq(&raw const *actual, &raw const **window));
                cancelled_owner(same_window, cancelled.as_ref().is_some_and(|check| check()))
            });
            if should_abort {
                application.abortModal();
            }
        });
    }
}
fn cancelled_owner(same_window: bool, cancelled: bool) -> bool {
    same_window && cancelled
}
fn current_request(expected: u64, current: u64) -> bool {
    current != u64::MAX && expected == current
}
fn matches_target(expected: u64, current: u64, same_window: bool) -> bool {
    expected != 0 && expected == current && same_window
}

#[cfg(test)]
mod tests {
    use super::{current_request, matches_target};
    #[test]
    fn stale_or_nested_unrelated_modal_never_receives_an_abort() {
        assert!(matches_target(7, 7, true));
        assert!(!matches_target(7, 8, true));
        assert!(!matches_target(7, 7, false));
        assert!(!matches_target(0, 0, true));
    }
    #[test]
    fn cancelled_owner_abort_preserves_live_successor_and_unrelated_nested_window() {
        assert!(super::cancelled_owner(true, true));
        assert!(!super::cancelled_owner(true, false));
        assert!(!super::cancelled_owner(false, true));
    }
    #[test]
    fn cancellation_before_native_queue_delivery_rejects_the_old_request() {
        assert!(current_request(7, 7));
        assert!(!current_request(7, 8));
        assert!(!current_request(u64::MAX, u64::MAX));
    }
    #[test]
    fn revoke_and_registration_share_one_atomic_owner_transition() -> Result<(), ipc_types::IpcError>
    {
        let mut registry = super::Registry::default();
        let old_request = registry.revocation;
        let old = registry.admit(old_request)?;
        let abort_target = registry.revoke();
        assert_eq!(abort_target, old);
        assert!(registry.admit(old_request).is_err());
        let successor = registry.admit(registry.revocation)?;
        assert!(!matches_target(abort_target, successor, true));
        Ok(())
    }
    #[test]
    fn stale_wallet_epoch_cannot_retire_a_newer_prompt_request() -> Result<(), ipc_types::IpcError>
    {
        let mut registry = super::Registry::default();
        let current = registry.admit(0)?;
        assert!(registry.revoke_if(|| false).is_none());
        assert_eq!(registry.revocation, 0);
        assert_eq!(registry.current, current);
        assert!(registry.admit(0).is_ok());
        Ok(())
    }
    #[test]
    fn queued_prompt_cannot_outlive_its_job_or_cancel_a_successor_job()
    -> Result<(), Box<dyn std::error::Error>> {
        let work = wallet_core::PasswordVaultWork::new();
        let job = work.begin()?;
        let queued = super::ModalRequest::for_job(&job);
        assert!(queued.check().is_ok());
        work.cancel();
        assert!(queued.check().is_err());
        drop(job);
        let next = work.begin()?;
        let successor = super::ModalRequest::for_job(&next);
        assert!(successor.check().is_ok());
        assert!(queued.check().is_err());
        drop(next);
        assert!(successor.check().is_err());
        Ok(())
    }
    #[test]
    fn already_cancelled_transfer_cannot_capture_a_new_generation_and_present() {
        let requested = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let cancel = crate::wallet_lifecycle::TransferCancellation::new(requested);
        let request = super::ModalRequest::for_transfer(&cancel);
        assert!(request.check().is_err());
    }
}
