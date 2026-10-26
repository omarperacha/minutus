use std::{error::Error, fmt};

/// Closed failure categories for the in-memory vault session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultSessionError {
    /// The inactivity timeout is outside the supported closed interval.
    InvalidInactivityTimeout,
    /// Unlock was requested while the vault was already unlocked.
    AlreadyUnlocked,
    /// The operation requires an unlocked vault.
    Locked,
    /// Local authorization was issued for a different vault.
    AuthorizationForDifferentVault,
    /// The supplied monotonic observation predates the prior observation.
    ClockMovedBackwards,
}

impl fmt::Display for VaultSessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidInactivityTimeout => "vault inactivity timeout is invalid",
            Self::AlreadyUnlocked => "vault is already unlocked",
            Self::Locked => "vault is locked",
            Self::AuthorizationForDifferentVault => {
                "local authorization belongs to a different vault"
            }
            Self::ClockMovedBackwards => "monotonic vault clock moved backwards",
        })
    }
}

impl Error for VaultSessionError {}
