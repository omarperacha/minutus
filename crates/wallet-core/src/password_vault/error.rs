use std::{error::Error, fmt};

/// Content-free password-vault failures. No untrusted parser/platform text escapes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PasswordVaultError {
    /// Password violates the reviewed byte/character policy.
    InvalidPassword,
    /// Format, algorithm, parameter, encoding or size is invalid.
    InvalidDocument,
    /// This version is deliberately unsupported, with no Keychain fallback.
    UnsupportedVersion,
    /// Authentication failed; wrong password and corruption are indistinguishable.
    AuthenticationFailed,
    /// A bounded allocation or cryptographic operation failed.
    CryptographyUnavailable,
    /// Operating-system entropy was unavailable.
    RandomnessUnavailable,
    /// Another password operation is still running or being discarded.
    Busy,
    /// A bounded local authentication backoff is still active.
    RateLimited,
    /// Lock/cancellation made this operation stale.
    Cancelled,
    /// A private filesystem operation failed.
    StorageUnavailable,
    /// Store ownership, permissions or a path/entry type is unsafe.
    UnsafeStorage,
    /// The destination exists or its exact source revision changed.
    Conflict,
    /// No such test wallet exists.
    NotFound,
    /// Publication occurred but its durability could not be confirmed.
    CommitUncertain,
    /// The platform has not qualified this storage implementation.
    UnsupportedPlatform,
}

impl fmt::Display for PasswordVaultError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidPassword => {
                "Use at least 15 characters, up to 1024 UTF-8 bytes; spaces and Unicode are allowed"
            }
            Self::InvalidDocument => "Password wallet document is invalid",
            Self::UnsupportedVersion => {
                "Wallet format is unsupported; the original has been left untouched"
            }
            Self::AuthenticationFailed => "Password wallet could not be authenticated",
            Self::CryptographyUnavailable => "Password wallet cryptography is unavailable",
            Self::RandomnessUnavailable => "Password wallet randomness is unavailable",
            Self::Busy => "Another password wallet operation is still running",
            Self::RateLimited => "Wait briefly before another password attempt",
            Self::Cancelled => "Password wallet operation was cancelled or locked",
            Self::StorageUnavailable => "Password wallet storage is unavailable",
            Self::UnsafeStorage => "Password wallet storage has unsafe paths or permissions",
            Self::Conflict => "Password wallet changed; start again with the current wallet",
            Self::NotFound => "Password wallet was not found",
            Self::CommitUncertain => {
                "Wallet commit durability is uncertain; lock and reopen before continuing"
            }
            Self::UnsupportedPlatform => {
                "Password wallet storage is not qualified on this platform"
            }
        })
    }
}
impl Error for PasswordVaultError {}
