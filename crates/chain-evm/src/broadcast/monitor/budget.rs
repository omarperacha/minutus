//! A single caller allowance spans every RPC in this observation attempt.
use super::{MonitorCause, MonitorFailure, MonitorStage};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
pub(super) const MAX_RPC: Duration = Duration::from_secs(12);
pub(super) struct Budget<'a> {
    started: Instant,
    remaining: Option<Duration>,
    cancelled: &'a AtomicBool,
}
impl<'a> Budget<'a> {
    pub(super) fn new(remaining: Option<Duration>, cancelled: &'a AtomicBool) -> Self {
        Self {
            started: Instant::now(),
            remaining,
            cancelled,
        }
    }
    pub(super) fn failure(
        &self,
        stage: MonitorStage,
        cause: impl Into<MonitorCause>,
    ) -> MonitorFailure {
        MonitorFailure::new(stage, cause.into(), self.started.elapsed())
    }
    pub(super) fn check(&self, stage: MonitorStage) -> Result<(), MonitorFailure> {
        if self.cancelled.load(Ordering::Acquire) {
            Err(self.failure(stage, MonitorCause::Cancelled))
        } else if self
            .remaining
            .is_some_and(|budget| self.started.elapsed() >= budget)
        {
            Err(self.failure(stage, MonitorCause::BudgetElapsed))
        } else {
            Ok(())
        }
    }
    pub(super) fn timeout(&self, stage: MonitorStage) -> Result<Duration, MonitorFailure> {
        self.check(stage)?;
        let timeout = self.remaining.map_or(MAX_RPC, |budget| {
            budget.saturating_sub(self.started.elapsed()).min(MAX_RPC)
        });
        if timeout.is_zero() {
            Err(self.failure(stage, MonitorCause::BudgetElapsed))
        } else {
            Ok(timeout)
        }
    }
}
