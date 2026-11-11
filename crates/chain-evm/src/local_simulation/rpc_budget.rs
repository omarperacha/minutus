//! A fixed network allowance inside the original whole-operation control.
//! This never starts/renews a control and cannot be selected by a caller/model.
use evm_simulation::{SimulationControl, SimulationError};
use std::time::Duration;
const MAX_IO: Duration = Duration::from_secs(3);
pub(super) fn timeout(control: &SimulationControl<'_>) -> Result<Duration, SimulationError> {
    cap(control.remaining()?)
}
fn cap(remaining: Duration) -> Result<Duration, SimulationError> {
    if remaining.is_zero() {
        Err(SimulationError::Limit)
    } else {
        Ok(remaining.min(MAX_IO))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    #[test]
    fn fixed_cap_cannot_expand_the_original_or_a_smaller_remaining_budget()
    -> Result<(), SimulationError> {
        assert_eq!(cap(Duration::from_secs(15))?, Duration::from_secs(3));
        assert_eq!(cap(Duration::MAX)?, Duration::from_secs(3));
        assert_eq!(cap(Duration::from_millis(17))?, Duration::from_millis(17));
        assert_eq!(cap(Duration::from_nanos(1))?, Duration::from_nanos(1));
        assert_eq!(cap(Duration::ZERO), Err(SimulationError::Limit));
        Ok(())
    }
    #[test]
    fn cancellation_is_checked_before_another_network_allowance_is_issued()
    -> Result<(), SimulationError> {
        let cancelled = AtomicBool::new(false);
        let control = SimulationControl::start(&cancelled);
        assert!(timeout(&control)? <= Duration::from_secs(3));
        cancelled.store(true, Ordering::Release);
        assert_eq!(timeout(&control), Err(SimulationError::Cancelled));
        Ok(())
    }
}
