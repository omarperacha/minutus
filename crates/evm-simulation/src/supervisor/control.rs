//! Fixed run budget and terminal quarantine after unconfirmed offline cleanup.
use crate::{SimulationError, resources};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
static QUARANTINED: AtomicBool = AtomicBool::new(false);
/// Rejects further proof-broker/worker operations after an offline child could
/// not be confirmed reaped. There is deliberately no runtime reset API.
///
/// # Errors
/// Returns a fixed unavailable error after unconfirmed signed-role termination.
pub fn ensure_worker_runtime_available() -> Result<(), SimulationError> {
    if QUARANTINED.load(Ordering::Acquire) {
        Err(SimulationError::WorkerUnavailable)
    } else {
        Ok(())
    }
}
pub(super) fn quarantine() {
    QUARANTINED.store(true, Ordering::Release);
}
/// Deadline/cancellation context for the trusted state-acquisition adapter.
pub struct SimulationControl<'a> {
    pub(super) cancelled: &'a AtomicBool,
    pub(super) started: Instant,
}
impl<'a> SimulationControl<'a> {
    /// Starts the fixed total run budget before state acquisition.
    #[must_use]
    pub fn start(cancelled: &'a AtomicBool) -> Self {
        Self {
            cancelled,
            started: Instant::now(),
        }
    }
    /// Remaining total run allowance. Adapters must use this as their IO timeout.
    ///
    /// # Errors
    /// Rejects cancellation or expired execution before any new external query.
    pub fn remaining(&self) -> Result<Duration, SimulationError> {
        ensure_worker_runtime_available()?;
        if self.cancelled.load(Ordering::Acquire) {
            return Err(SimulationError::Cancelled);
        }
        resources::WALL
            .checked_sub(self.started.elapsed())
            .filter(|left| !left.is_zero())
            .ok_or(SimulationError::Limit)
    }
}
