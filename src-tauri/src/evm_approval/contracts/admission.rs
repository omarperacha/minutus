//! Private transport protocol. The caller extracts readiness from the sealed
//! candidate, selected wallet, current scope and cancellation at the callback.
//! This module creates no candidate, proof, approval or signing capability.
pub(super) fn run(
    ready: bool,
    reserve: impl FnOnce() -> bool,
    mark: impl FnOnce() -> bool,
    begin: impl FnOnce() -> bool,
    discard: impl FnOnce(bool),
) -> bool {
    if !ready || !reserve() {
        return false;
    }
    let marked = mark();
    let admitted = marked && begin();
    if !admitted {
        discard(marked);
    }
    admitted
}

pub(super) const fn may_have_sent(admitted: bool, entered: bool, returned: bool) -> bool {
    admitted || (entered && !returned)
}

#[cfg(test)]
mod tests {
    use super::{may_have_sent, run};
    use std::cell::RefCell;
    #[test]
    fn interrupted_callback_is_uncertain_and_normal_false_is_known_unsent() {
        assert!(may_have_sent(false, true, false));
        assert!(may_have_sent(true, true, true));
        assert!(!may_have_sent(false, true, true));
        assert!(!may_have_sent(false, false, false));
    }
    #[test]
    fn expired_cancelled_or_changed_scope_cannot_reserve_or_send() {
        let called = std::cell::Cell::new(0);
        assert!(!run(
            false,
            || {
                called.set(called.get() + 1);
                false
            },
            || {
                called.set(called.get() + 1);
                false
            },
            || {
                called.set(called.get() + 1);
                false
            },
            |_| called.set(called.get() + 1)
        ));
        assert_eq!(called.get(), 0);
    }
    #[test]
    fn changed_nonce_or_prerequisite_retires_only_the_unsent_reservation() {
        let log = RefCell::new(Vec::new());
        assert!(!run(
            true,
            || {
                log.borrow_mut().push("reserve");
                true
            },
            || {
                log.borrow_mut().push("mark-failed");
                false
            },
            || {
                log.borrow_mut().push("unexpected-send");
                false
            },
            |marked| {
                assert!(!marked);
                log.borrow_mut().push("reserved-cleanup");
            }
        ));
        assert_eq!(
            *log.borrow(),
            ["reserve", "mark-failed", "reserved-cleanup"]
        );
    }
    #[test]
    fn cancellation_at_begin_rolls_back_only_in_the_callback_false_path() {
        let log = RefCell::new(Vec::new());
        assert!(!run(
            true,
            || true,
            || {
                log.borrow_mut().push("possible-send");
                true
            },
            || {
                log.borrow_mut().push("begin-false");
                false
            },
            |marked| {
                assert!(marked);
                log.borrow_mut().push("known-unsent-cleanup");
            }
        ));
        assert_eq!(
            *log.borrow(),
            ["possible-send", "begin-false", "known-unsent-cleanup"]
        );
    }
    #[test]
    #[allow(
        clippy::panic,
        reason = "Injects a transport-owner panic to verify that durable possible-send state is never cleaned up during unwinding"
    )]
    fn admission_is_once_and_never_cleans_up_after_possible_send() {
        let log = RefCell::new(Vec::new());
        assert!(run(
            true,
            || {
                log.borrow_mut().push("reserve");
                true
            },
            || {
                log.borrow_mut().push("possible-send");
                true
            },
            || {
                log.borrow_mut().push("begin");
                true
            },
            |_| log.borrow_mut().push("unexpected-cleanup")
        ));
        assert_eq!(*log.borrow(), ["reserve", "possible-send", "begin"]);
        let cleaned = std::sync::atomic::AtomicBool::new(false);
        let panic = std::panic::catch_unwind(|| {
            run(
                true,
                || true,
                || true,
                || panic!("transport owner interrupted"),
                |_| cleaned.store(true, std::sync::atomic::Ordering::Release),
            )
        });
        assert!(panic.is_err());
        assert!(!cleaned.load(std::sync::atomic::Ordering::Acquire));
    }
}
