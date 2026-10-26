use std::{error::Error, fmt};

/// Closed failure categories for vault-key generation and platform storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultKeyStoreError {
    /// A vault key identifier violates its bounded format.
    InvalidKeyId,
    /// The operating system could not provide cryptographic randomness.
    RandomnessUnavailable,
    /// No non-synchronizing key exists for the requested vault.
    NotFound,
    /// User presence was denied or could not be established.
    AuthorizationDenied,
    /// The user cancelled the owner-authorization prompt.
    AuthorizationCancelled,
    /// The protected Keychain could not present owner authorization.
    AuthorizationUnavailable,
    /// The caller lacks a valid signed application identity or Keychain entitlement.
    SigningConfigurationUnavailable,
    /// Stored bytes do not contain one complete 256-bit key.
    InvalidStoredKey,
    /// The platform Keychain operation failed without exposing private details.
    PlatformFailure,
}

impl fmt::Display for VaultKeyStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidKeyId => "vault key identifier is invalid",
            Self::RandomnessUnavailable => "cryptographic randomness is unavailable",
            Self::NotFound => "vault encryption key was not found",
            Self::AuthorizationDenied => "vault key authorization was denied",
            Self::AuthorizationCancelled => "vault key authorization was cancelled",
            Self::AuthorizationUnavailable => "vault key authorization is unavailable",
            Self::SigningConfigurationUnavailable => "wallet application signing is not configured",
            Self::InvalidStoredKey => "stored vault encryption key is invalid",
            Self::PlatformFailure => "platform vault key storage failed",
        })
    }
}

impl Error for VaultKeyStoreError {}
