use std::time::{Duration, Instant};

use crate::{
    VAULT_ENCRYPTION_KEY_BYTES, VaultEncryptionKey, VaultKeyId, VaultSigningAuthorization,
    VaultUnlockAuthorization,
};

use super::VaultSessionError;

/// Smallest supported automatic-lock interval.
pub const MIN_INACTIVITY_TIMEOUT: Duration = Duration::from_secs(1);
/// Largest supported automatic-lock interval.
pub const MAX_INACTIVITY_TIMEOUT: Duration = Duration::from_hours(24);

/// Validated automatic-lock interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VaultInactivityTimeout(Duration);

impl VaultInactivityTimeout {
    /// Validates a configured automatic-lock interval.
    ///
    /// # Errors
    ///
    /// Returns [`VaultSessionError::InvalidInactivityTimeout`] unless the
    /// interval is between one second and 24 hours, inclusive.
    pub fn new(duration: Duration) -> Result<Self, VaultSessionError> {
        if !(MIN_INACTIVITY_TIMEOUT..=MAX_INACTIVITY_TIMEOUT).contains(&duration) {
            return Err(VaultSessionError::InvalidInactivityTimeout);
        }
        Ok(Self(duration))
    }

    /// Returns the validated interval.
    #[must_use]
    pub const fn get(self) -> Duration {
        self.0
    }
}

/// Reason the in-memory vault key is unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultLockReason {
    /// The vault has not yet been unlocked in this process.
    NeverUnlocked,
    /// The user explicitly locked the vault.
    Manual,
    /// The configured inactivity interval elapsed.
    Inactivity,
    /// The host reported that the device is going to sleep.
    SystemSleep,
    /// An explicitly confirmed deletion is removing the vault.
    Deletion,
    /// Monotonic observations were inconsistent, so the session failed closed.
    ClockDiscontinuity,
}

/// Public, secret-free vault session status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultSessionStatus {
    /// The vault is locked for the given reason.
    Locked(VaultLockReason),
    /// The in-memory vault key is available to trusted wallet operations.
    Unlocked,
}

enum SessionState {
    Locked(VaultLockReason),
    Unlocked {
        key: VaultEncryptionKey,
        last_activity: Instant,
    },
}

/// Wallet-owned in-memory vault-key lifecycle.
///
/// The session intentionally implements neither cloning, debugging nor
/// serialization. Every transition to a locked state drops and zeroizes the
/// in-memory vault key.
pub struct VaultSession {
    key_id: VaultKeyId,
    inactivity_timeout: VaultInactivityTimeout,
    state: SessionState,
}

impl VaultSession {
    /// Creates a locked session for one exact vault.
    #[must_use]
    pub fn new(key_id: VaultKeyId, inactivity_timeout: VaultInactivityTimeout) -> Self {
        Self {
            key_id,
            inactivity_timeout,
            state: SessionState::Locked(VaultLockReason::NeverUnlocked),
        }
    }

    /// Returns the non-secret vault identifier.
    #[must_use]
    pub fn key_id(&self) -> &VaultKeyId {
        &self.key_id
    }

    /// Returns the current secret-free status without changing inactivity state.
    #[must_use]
    pub const fn status(&self) -> VaultSessionStatus {
        match self.state {
            SessionState::Locked(reason) => VaultSessionStatus::Locked(reason),
            SessionState::Unlocked { .. } => VaultSessionStatus::Unlocked,
        }
    }

    /// Unlocks using a one-use capability issued after local authorization.
    ///
    /// # Errors
    ///
    /// Returns a closed error when already unlocked or when the authorization
    /// belongs to a different vault.
    pub fn unlock(
        &mut self,
        authorization: VaultUnlockAuthorization,
        now: Instant,
    ) -> Result<(), VaultSessionError> {
        if matches!(self.state, SessionState::Unlocked { .. }) {
            return Err(VaultSessionError::AlreadyUnlocked);
        }
        let (key_id, key) = authorization.into_parts();
        if key_id != self.key_id {
            return Err(VaultSessionError::AuthorizationForDifferentVault);
        }
        self.state = SessionState::Unlocked {
            key,
            last_activity: now,
        };
        Ok(())
    }

    /// Immediately locks at the user's request.
    pub fn lock_manually(&mut self) {
        self.lock(VaultLockReason::Manual);
    }

    /// Immediately locks when the host reports system sleep.
    pub fn handle_system_sleep(&mut self) {
        self.lock(VaultLockReason::SystemSleep);
    }

    pub(crate) fn lock_for_deletion(&mut self) {
        self.lock(VaultLockReason::Deletion);
    }

    /// Applies the inactivity policy at a host-supplied monotonic observation.
    ///
    /// A backwards observation fails closed and locks the session.
    #[must_use]
    pub fn enforce_inactivity(&mut self, now: Instant) -> VaultSessionStatus {
        let SessionState::Unlocked { last_activity, .. } = &self.state else {
            return self.status();
        };
        let Some(inactive_for) = now.checked_duration_since(*last_activity) else {
            self.lock(VaultLockReason::ClockDiscontinuity);
            return self.status();
        };
        if inactive_for >= self.inactivity_timeout.get() {
            self.lock(VaultLockReason::Inactivity);
        }
        self.status()
    }

    /// Records trusted user activity after first enforcing automatic lock.
    ///
    /// # Errors
    ///
    /// Returns [`VaultSessionError::Locked`] when already locked or when the
    /// inactivity deadline has elapsed. A backwards observation locks the
    /// session and returns [`VaultSessionError::ClockMovedBackwards`].
    pub fn record_activity(&mut self, now: Instant) -> Result<(), VaultSessionError> {
        self.require_unexpired(now)?;
        let SessionState::Unlocked { last_activity, .. } = &mut self.state else {
            return Err(VaultSessionError::Locked);
        };
        *last_activity = now;
        Ok(())
    }

    /// Runs one trusted wallet operation while unlocked and records activity.
    ///
    /// The borrowed key cannot escape the callback. This path does not grant
    /// signing authority; signing requires [`Self::authorize_signing`].
    ///
    /// # Errors
    ///
    /// Returns a closed lock or clock error before invoking the callback.
    pub fn with_unlocked_key<R>(
        &mut self,
        now: Instant,
        operation: impl FnOnce(&[u8; VAULT_ENCRYPTION_KEY_BYTES]) -> R,
    ) -> Result<R, VaultSessionError> {
        self.record_activity(now)?;
        let SessionState::Unlocked { key, .. } = &self.state else {
            return Err(VaultSessionError::Locked);
        };
        Ok(operation(key.expose_secret()))
    }

    /// Consumes fresh signing authorization for an unlocked matching vault.
    ///
    /// Signing authority cannot be derived from the cached unlock key. The
    /// caller must obtain this distinct one-use capability through fresh local
    /// authentication immediately before signing.
    ///
    /// # Errors
    ///
    /// Returns a closed lock, clock or wrong-vault error.
    pub fn authorize_signing(
        &mut self,
        authorization: VaultSigningAuthorization,
        now: Instant,
    ) -> Result<VaultSigningPermit, VaultSessionError> {
        self.require_unexpired(now)?;
        let (key_id, key) = authorization.into_parts();
        if key_id != self.key_id {
            return Err(VaultSessionError::AuthorizationForDifferentVault);
        }
        let SessionState::Unlocked { last_activity, .. } = &mut self.state else {
            return Err(VaultSessionError::Locked);
        };
        *last_activity = now;
        Ok(VaultSigningPermit { key_id, key })
    }

    fn require_unexpired(&mut self, now: Instant) -> Result<(), VaultSessionError> {
        let prior_status = self.status();
        let status = self.enforce_inactivity(now);
        match status {
            VaultSessionStatus::Unlocked => Ok(()),
            VaultSessionStatus::Locked(VaultLockReason::ClockDiscontinuity)
                if prior_status == VaultSessionStatus::Unlocked =>
            {
                Err(VaultSessionError::ClockMovedBackwards)
            }
            VaultSessionStatus::Locked(_) => Err(VaultSessionError::Locked),
        }
    }

    fn lock(&mut self, reason: VaultLockReason) {
        self.state = SessionState::Locked(reason);
    }
}

/// One-use signing capability backed by a freshly authorized vault key.
///
/// It intentionally implements neither cloning, debugging nor serialization.
/// Dropping it zeroizes its key.
pub struct VaultSigningPermit {
    key_id: VaultKeyId,
    key: VaultEncryptionKey,
}

impl VaultSigningPermit {
    pub(crate) fn key_id(&self) -> &VaultKeyId {
        &self.key_id
    }

    /// Runs exactly one signing operation and then zeroizes the authorized key.
    pub(crate) fn sign<R>(
        self,
        operation: impl FnOnce(&[u8; VAULT_ENCRYPTION_KEY_BYTES]) -> R,
    ) -> R {
        operation(self.key.expose_secret())
    }
}
