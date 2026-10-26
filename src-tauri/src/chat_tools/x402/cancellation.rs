//! Bridge request cancellation into network waits and the pre-signing boundary.

use std::sync::atomic::{AtomicBool, Ordering};
use web_retrieval::WebCancellation;

pub(super) fn run<T>(
    cancelled: &AtomicBool,
    network: &WebCancellation,
    operation: impl FnOnce() -> T,
) -> T {
    let finished = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            while !finished.load(Ordering::Acquire) {
                if cancelled.load(Ordering::Acquire) {
                    network.cancel();
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        });
        let _finish = Finish(&finished);
        operation()
    })
}
struct Finish<'a>(&'a AtomicBool);
impl Drop for Finish<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_propagates_while_an_operation_waits() {
        let cancelled = AtomicBool::new(true);
        let network = WebCancellation::default();
        run(&cancelled, &network, || {
            for _ in 0..100 {
                if network.is_cancelled() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            assert!(network.is_cancelled());
        });
    }
}
