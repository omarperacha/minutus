use super::PasswordVaultError;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

struct State {
    active: bool,
    failures: u32,
    not_before: Instant,
}

/// Admission/cancellation state for one native password-vault owner.
/// Cancellation invalidates results, but never claims to interrupt Argon2.
#[derive(Clone)]
pub struct PasswordVaultWork(Arc<WorkState>);
struct WorkState {
    state: Mutex<State>,
    generation: AtomicU64,
}

impl Default for PasswordVaultWork {
    fn default() -> Self {
        Self::new()
    }
}

impl PasswordVaultWork {
    /// Creates idle bounded-work admission.
    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(WorkState {
            generation: AtomicU64::new(0),
            state: Mutex::new(State {
                active: false,
                failures: 0,
                not_before: Instant::now(),
            }),
        }))
    }

    /// Admits exactly one job, retaining admission until its scratch/key owners drop.
    ///
    /// # Errors
    /// Returns busy while an earlier job is running, including cancelled work.
    pub fn begin(&self) -> Result<PasswordVaultJob, PasswordVaultError> {
        let mut state = self
            .0
            .state
            .lock()
            .map_err(|_| PasswordVaultError::Cancelled)?;
        if state.active {
            return Err(PasswordVaultError::Busy);
        }
        if Instant::now() < state.not_before {
            return Err(PasswordVaultError::RateLimited);
        }
        let generation = self
            .0
            .generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        state.active = true;
        Ok(PasswordVaultJob {
            state: self.0.clone(),
            generation,
        })
    }

    /// Invalidates current work immediately. A bounded blocking KDF still finishes
    /// and wipes its buffers before another job can begin.
    pub fn cancel(&self) {
        self.0.generation.fetch_add(1, Ordering::AcqRel);
    }
}

/// Non-cloneable job identity; no credentials, serialisation or approval authority.
pub struct PasswordVaultJob {
    state: Arc<WorkState>,
    generation: u64,
}

impl PasswordVaultJob {
    pub(crate) fn authentication_failed(&self) {
        if let Ok(mut state) = self.state.state.lock()
            && self.state.generation.load(Ordering::Acquire) == self.generation
        {
            state.failures = state.failures.saturating_add(1).min(5);
            state.not_before = Instant::now() + Duration::from_secs(1_u64 << state.failures);
        }
    }
    pub(crate) fn authentication_succeeded(&self) {
        if let Ok(mut state) = self.state.state.lock()
            && self.state.generation.load(Ordering::Acquire) == self.generation
        {
            state.failures = 0;
            state.not_before = Instant::now();
        }
    }
    /// Rejects output from cancelled or superseded work.
    ///
    /// # Errors
    /// Returns cancelled if the host locked/cancelled this job.
    pub fn check(&self) -> Result<(), PasswordVaultError> {
        if self.state.generation.load(Ordering::Acquire) == self.generation {
            Ok(())
        } else {
            Err(PasswordVaultError::Cancelled)
        }
    }

    /// Checks cancellation before and after one bounded state/publication effect.
    /// Revocation never waits on this callback. An already-started filesystem
    /// publication may commit; its owner must classify post-publication cancellation
    /// as commit-uncertain. Never run native prompts or KDFs here.
    /// # Errors
    /// Rejects stale work and never returns a successful stale capability.
    pub fn with_current<T>(
        &self,
        effect: impl FnOnce() -> Result<T, PasswordVaultError>,
    ) -> Result<T, PasswordVaultError> {
        self.check()?;
        let result = effect()?;
        self.check()?;
        Ok(result)
    }
}

impl Drop for PasswordVaultJob {
    fn drop(&mut self) {
        if let Ok(mut state) = self.state.state.lock() {
            state.active = false;
        }
    }
}

/// Private identity witness for transferring fresh authentication off the blocking executor.
pub(crate) struct JobWitness {
    state: Arc<WorkState>,
    generation: u64,
}
impl PasswordVaultJob {
    pub(crate) fn witness(&self) -> JobWitness {
        JobWitness {
            state: self.state.clone(),
            generation: self.generation,
        }
    }
}
impl JobWitness {
    pub(crate) fn matches(&self, job: &PasswordVaultJob) -> bool {
        Arc::ptr_eq(&self.state, &job.state)
            && self.generation == job.generation
            && job.check().is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn revocation_does_not_wait_for_a_blocked_effect_and_rejects_its_result()
    -> Result<(), Box<dyn std::error::Error>> {
        let work = PasswordVaultWork::new();
        let job = work.begin()?;
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (finish_tx, finish_rx) = std::sync::mpsc::channel();
        let running = std::thread::spawn(move || {
            job.with_current(|| {
                started_tx
                    .send(())
                    .map_err(|_| PasswordVaultError::Cancelled)?;
                finish_rx
                    .recv()
                    .map_err(|_| PasswordVaultError::Cancelled)?;
                Ok(())
            })
        });
        started_rx.recv_timeout(Duration::from_secs(2))?;
        let (cancelled_tx, cancelled_rx) = std::sync::mpsc::channel();
        let cancelling = work.clone();
        let cancel = std::thread::spawn(move || {
            cancelling.cancel();
            let _ = cancelled_tx.send(());
        });
        let immediate = cancelled_rx.recv_timeout(Duration::from_secs(1)).is_ok();
        finish_tx.send(())?;
        cancel.join().map_err(|_| "cancel panic")?;
        assert!(
            immediate,
            "revocation blocked behind a filesystem/state effect"
        );
        assert_eq!(
            running.join().map_err(|_| "effect panic")?,
            Err(PasswordVaultError::Cancelled)
        );
        Ok(())
    }
    #[test]
    fn failed_authentication_throttles_new_jobs_without_sleeping_or_claiming_offline_protection()
    -> Result<(), PasswordVaultError> {
        let work = PasswordVaultWork::new();
        let job = work.begin()?;
        job.authentication_failed();
        drop(job);
        assert!(matches!(work.begin(), Err(PasswordVaultError::RateLimited)));
        Ok(())
    }
    #[test]
    fn cancellation_retains_admission_and_rejects_late_publication()
    -> Result<(), PasswordVaultError> {
        let work = PasswordVaultWork::new();
        let job = work.begin()?;
        assert!(matches!(work.begin(), Err(PasswordVaultError::Busy)));
        work.cancel();
        assert!(matches!(work.begin(), Err(PasswordVaultError::Busy)));
        let mut published = false;
        assert_eq!(
            job.with_current(|| {
                published = true;
                Ok(())
            }),
            Err(PasswordVaultError::Cancelled)
        );
        assert!(!published);
        drop(job);
        work.begin()?.check()?;
        Ok(())
    }
}
