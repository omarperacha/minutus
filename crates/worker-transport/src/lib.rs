//! Fixed-role authenticated worker control. Sensitive payloads travel only over
//! anonymous pipes received after peer authentication and broker confinement.
mod identity;
mod native;
mod session;
pub use session::{ReadyChannel, ReapedWorker};
use std::{error, fmt, sync::atomic::AtomicBool, time::Duration};

/// Build-pinned independently isolated execution role; never a model selector.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    /// Exact official-source compiler helper.
    Compiler,
    /// Public unsigned execution discovery helper.
    EvmOnline,
    /// Frozen signed native Base execution helper.
    BaseOffline,
}
impl Role {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Compiler => "compiler",
            Self::EvmOnline => "evm-online",
            Self::BaseOffline => "base-offline",
        }
    }
    pub(crate) const fn wall(self) -> Duration {
        match self {
            Self::Compiler => Duration::from_mins(1),
            Self::EvmOnline | Self::BaseOffline => Duration::from_secs(15),
        }
    }
}
/// Closed transport failures, without child-selected text or process details.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    /// Missing pins, invalid packaged identity or unavailable service.
    Unavailable,
    /// A fixed protocol/one-shot ownership invariant failed.
    Protocol,
    /// Host-owned cancellation was observed.
    Cancelled,
    /// The original bounded run deadline expired.
    Limit,
    /// No authenticated actual-worker reaping proof was obtained.
    UnconfirmedTermination,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Worker transport failed: {self:?}")
    }
}
impl error::Error for Error {}
/// Exact loaded-host pins for one role. No raw path/hash constructor exists.
pub struct PackagedWorker {
    identity: identity::Identity,
    role: Role,
}
impl PackagedWorker {
    /// Reads immutable loaded-host broker/worker pins and verifies fixed files.
    /// # Errors
    /// Rejects unsupported hosts, changed/indirect files or incomplete pins.
    pub fn packaged(role: Role) -> Result<Self, Error> {
        Ok(Self {
            identity: identity::Identity::packaged(role)?,
            role,
        })
    }
    /// Exact worker SHA256 from the loaded transport cohort, for the existing
    /// upper-owner loaded-image pin cross-check; this grants no construction API.
    #[must_use]
    pub fn worker_sha256(&self) -> &str {
        &self.identity.worker_sha
    }
    /// Exact worker code-directory hash from the same loaded transport cohort.
    #[must_use]
    pub fn worker_cdhash(&self) -> &str {
        &self.identity.worker_cdhash
    }
    /// Starts only the authenticated packaged XPC route. No direct fallback.
    /// # Errors
    /// Rejects cancellation, missing confinement/identity and startup deadlines.
    pub fn start(&self, cancelled: &AtomicBool) -> Result<ReadyChannel, Error> {
        self.start_with_deadline(self.role.wall(), cancelled)
    }
    /// Preserves the caller's remaining total budget, capped by the fixed role.
    /// # Errors
    /// Rejects the same startup failures and any exhausted caller budget.
    pub fn start_with_deadline(
        &self,
        timeout: Duration,
        cancelled: &AtomicBool,
    ) -> Result<ReadyChannel, Error> {
        let started = std::time::Instant::now();
        self.identity.verify()?;
        let timeout = timeout.checked_sub(started.elapsed()).ok_or(Error::Limit)?;
        session::start(
            &self.identity,
            self.role,
            timeout.min(self.role.wall()),
            cancelled,
        )
    }
}
