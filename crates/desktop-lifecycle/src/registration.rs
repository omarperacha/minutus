// The Objective-C notification bridge is isolated here as required by the
// workspace policy. See README.md for its threading and lifetime invariants.
#![allow(unsafe_code)]

use block2::RcBlock;
use objc2::{
    MainThreadMarker,
    rc::Retained,
    runtime::{NSObjectProtocol, ProtocolObject},
};
use objc2_app_kit::{
    NSApplicationDidResignActiveNotification, NSWorkspace, NSWorkspaceScreensDidSleepNotification,
    NSWorkspaceSessionDidResignActiveNotification, NSWorkspaceWillSleepNotification,
};
use objc2_foundation::{
    NSDistributedNotificationCenter, NSNotification, NSNotificationCenter, NSNotificationName,
    NSString,
};
use std::{
    cell::RefCell,
    fmt,
    panic::{AssertUnwindSafe, catch_unwind},
    ptr::NonNull,
    sync::Arc,
};

struct Observer {
    center: Retained<NSNotificationCenter>,
    token: Retained<ProtocolObject<dyn NSObjectProtocol>>,
}
impl Observer {
    fn new(
        center: Retained<NSNotificationCenter>,
        name: &NSNotificationName,
        callback: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let block = RcBlock::new(move |_: NonNull<NSNotification>| {
            // Never unwind over Objective-C, including a poisoned host lock.
            let _ = catch_unwind(AssertUnwindSafe(|| callback()));
        });
        // SAFETY: nil object accepts every sender and has no typed payload. A
        // nil queue delivers synchronously on the posting thread. The block
        // captures only a Send+Sync callback; it neither touches AppKit nor
        // dereferences the borrowed notification. Foundation copies the block
        // for the observer lifetime. The returned token and center stay owned.
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(Some(name), None, None, &block)
        };
        Self { center, token }
    }
}
impl Drop for Observer {
    fn drop(&mut self) {
        // SAFETY: exactly the token returned by this center is removed while
        // both remain retained. No notification payload or callback is borrowed.
        unsafe {
            self.center.removeObserver((*self.token).as_ref());
        }
    }
}
thread_local! {static OBSERVERS:RefCell<Vec<Observer>>=const {RefCell::new(Vec::new())};}

/// A closed registration error without native diagnostic or payload content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LifecycleRegistrationError;
impl fmt::Display for LifecycleRegistrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("lifecycle registration requires the main thread and a single owner")
    }
}
impl std::error::Error for LifecycleRegistrationError {}

/// Non-secret lifecycle reason; the host decides which operation to invalidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleEvent {
    /// Another application became active; the host may preserve its session.
    ApplicationInactive,
    /// Screen lock/unlock, sleep or session resignation requires locking.
    SecurityBoundary,
}

fn screen_lock_observers(
    center: &Retained<NSNotificationCenter>,
    callback: &Arc<dyn Fn(LifecycleEvent) + Send + Sync>,
) -> [Observer; 2] {
    // These macOS names are compatibility assumptions, not authenticated state.
    // Treat both directions solely as revocation: a quick lock/unlock must not
    // resurrect pending authority, and a spoofed message can only force a lock.
    ["com.apple.screenIsLocked", "com.apple.screenIsUnlocked"].map(|name| {
        let callback = callback.clone();
        Observer::new(
            center.clone(),
            &NSString::from_str(name),
            Arc::new(move || callback(LifecycleEvent::SecurityBoundary)),
        )
    })
}

/// Installs one process-lifetime observer set on the main thread.
///
/// The callback may run on any posting thread and must perform only short,
/// non-blocking invalidation; schedule native presentation back to the main thread.
///
/// # Errors
/// Rejects off-main-thread or duplicate registration.
pub fn install_lock_notifications(
    callback: impl Fn(LifecycleEvent) + Send + Sync + 'static,
) -> Result<(), LifecycleRegistrationError> {
    let _mtm = MainThreadMarker::new().ok_or(LifecycleRegistrationError)?;
    OBSERVERS.with(|slot| {
        let mut observers = slot.borrow_mut();
        if !observers.is_empty() {
            return Err(LifecycleRegistrationError);
        }
        let callback: Arc<dyn Fn(LifecycleEvent) + Send + Sync> = Arc::new(callback);
        let workspace = NSWorkspace::sharedWorkspace().notificationCenter();
        // SAFETY: these immutable framework-defined notification names are
        // available on every supported macOS version. No mutable static access.
        let names = unsafe {
            [
                NSWorkspaceWillSleepNotification,
                NSWorkspaceScreensDidSleepNotification,
                NSWorkspaceSessionDidResignActiveNotification,
            ]
        };
        for name in names {
            let callback = callback.clone();
            observers.push(Observer::new(
                workspace.clone(),
                name,
                Arc::new(move || callback(LifecycleEvent::SecurityBoundary)),
            ));
        }
        // Foundation retains the concrete distributed center through the safe
        // superclass upcast; removal still dispatches to that same center.
        observers.extend(screen_lock_observers(
            &NSDistributedNotificationCenter::defaultCenter().into_super(),
            &callback,
        ));
        // SAFETY: same immutable system constant invariant as above.
        let inactive = unsafe { NSApplicationDidResignActiveNotification };
        observers.push(Observer::new(
            NSNotificationCenter::defaultCenter(),
            inactive,
            Arc::new(move || callback(LifecycleEvent::ApplicationInactive)),
        ));
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[test]
    fn registration_retains_callback_until_observer_is_removed() {
        let center = NSNotificationCenter::new();
        let name = NSString::from_str("MinutusSyntheticLifecycleTest");
        let count = Arc::new(AtomicUsize::new(0));
        let callback_count = count.clone();
        let observer = Observer::new(
            center.clone(),
            &name,
            Arc::new(move || {
                callback_count.fetch_add(1, Ordering::SeqCst);
            }),
        );
        // SAFETY: local synthetic name, no object or typed payload.
        unsafe {
            center.postNotificationName_object(&name, None);
        }
        assert_eq!(count.load(Ordering::SeqCst), 1);
        drop(observer);
        // SAFETY: same local notification, after unregistration.
        unsafe {
            center.postNotificationName_object(&name, None);
        }
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn brief_lock_unlock_pair_revokes_twice_and_observers_unregister() {
        // Use a private in-process center: never broadcast OS-looking events
        // to other applications or pretend this proves real macOS delivery.
        let center = NSNotificationCenter::new();
        let count = Arc::new(AtomicUsize::new(0));
        let callback_count = count.clone();
        let callback: Arc<dyn Fn(LifecycleEvent) + Send + Sync> = Arc::new(move |event| {
            if event == LifecycleEvent::SecurityBoundary {
                callback_count.fetch_add(1, Ordering::SeqCst);
            }
        });
        let observers = screen_lock_observers(&center, &callback);
        for name in ["com.apple.screenIsLocked", "com.apple.screenIsUnlocked"] {
            // SAFETY: private center, exact local name, no object or payload.
            unsafe {
                center.postNotificationName_object(&NSString::from_str(name), None);
            }
        }
        assert_eq!(count.load(Ordering::SeqCst), 2);
        drop(observers);
        // SAFETY: same private center and no payload, after unregistration.
        unsafe {
            center
                .postNotificationName_object(&NSString::from_str("com.apple.screenIsLocked"), None);
        }
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }
}
