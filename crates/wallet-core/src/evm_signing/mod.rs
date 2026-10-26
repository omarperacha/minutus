//! Wallet-owned EVM signing gated by exact approval and fresh user presence.

use std::{error::Error, fmt, str::FromStr as _, time::Instant};

use bip32::{
    DerivationPath, XPrv,
    secp256k1::ecdsa::{RecoveryId, Signature, SigningKey, VerifyingKey},
};
use transaction_policy::{
    ApprovalBindingInput, ApprovalCommitment, ApprovalCommitmentError, ApprovalConsumptionStore,
    ApprovalInvalidationReason, ApprovalTransactionType, ConsumedApproval, VerifiedApproval,
};
use zeroize::Zeroizing;

use crate::{
    EVM_DEFAULT_DERIVATION_PATH, EvmAddress, ImportedEvmPrivateKey, RecoverySeed, VaultKeyId,
    VaultKeyStoreError, VaultSession, VaultSessionError, VaultSessionStatus,
    VaultSigningAuthorization, VaultSigningPermit, evm_address::from_uncompressed_public_key,
};

#[cfg(target_os = "macos")]
use crate::MacOsVaultKeyStore;

const EVM_PRIVATE_KEY_BYTES: usize = 32;

/// Wallet-owned secp256k1 signing key for one exact vault and public account.
///
/// The key implements neither cloning, debugging, formatting nor serialization.
/// It can produce a signature only through the approval and user-presence gate.
pub struct EvmSigningKey {
    vault_id: VaultKeyId,
    account: EvmAddress,
    private_key: Zeroizing<[u8; EVM_PRIVATE_KEY_BYTES]>,
}

impl EvmSigningKey {
    /// Moves a validated imported private key into the wallet signing boundary.
    ///
    /// # Errors
    ///
    /// Returns a closed error if the key can no longer derive its public account.
    pub fn from_imported(
        vault_id: VaultKeyId,
        private_key: ImportedEvmPrivateKey,
    ) -> Result<Self, EvmSigningError> {
        let bytes = *private_key.expose_secret();
        drop(private_key);
        Self::from_bytes(vault_id, bytes)
    }

    /// Derives the default EVM signing key from a verified recovery seed.
    ///
    /// # Errors
    ///
    /// Returns a closed error if the fixed BIP-32 path or derived key is invalid.
    pub fn from_recovery_seed(
        vault_id: VaultKeyId,
        seed: &RecoverySeed,
    ) -> Result<Self, EvmSigningError> {
        let path = DerivationPath::from_str(EVM_DEFAULT_DERIVATION_PATH)
            .map_err(|_| EvmSigningError::KeyDerivationFailed)?;
        let extended = XPrv::derive_from_path(seed.expose_secret(), &path)
            .map_err(|_| EvmSigningError::KeyDerivationFailed)?;
        let bytes = extended.private_key().to_bytes();
        Self::from_bytes(vault_id, bytes.into())
    }

    /// Returns the public account controlled by this signing key.
    #[must_use]
    pub const fn account(&self) -> &EvmAddress {
        &self.account
    }

    /// Returns the non-secret vault identifier this key belongs to.
    #[must_use]
    pub const fn vault_id(&self) -> &VaultKeyId {
        &self.vault_id
    }

    fn from_bytes(
        vault_id: VaultKeyId,
        private_key: [u8; EVM_PRIVATE_KEY_BYTES],
    ) -> Result<Self, EvmSigningError> {
        let private_key = Zeroizing::new(private_key);
        let signing_key = SigningKey::from_slice(private_key.as_ref())
            .map_err(|_| EvmSigningError::InvalidPrivateKey)?;
        let public_key = signing_key.verifying_key().to_sec1_point(false);
        let account = from_uncompressed_public_key(public_key.as_bytes())
            .ok_or(EvmSigningError::AddressDerivationFailed)?;
        Ok(Self {
            vault_id,
            account,
            private_key,
        })
    }

    fn sign(
        &self,
        approval: &ConsumedApproval<'_>,
        permit: VaultSigningPermit,
    ) -> Result<EvmTransactionSignature, EvmSigningError> {
        self.validate_signing(approval.account(), approval.transaction_type(), &permit)?;

        permit.sign(|_freshly_authorized_vault_key| {
            let signing_key = SigningKey::from_slice(self.private_key.as_ref())
                .map_err(|_| EvmSigningError::InvalidPrivateKey)?;
            let (signature, recovery_id) =
                signing_key.sign_prehash_recoverable(approval.signing_digest());
            if recovery_id.is_x_reduced() {
                return Err(EvmSigningError::SignatureFailed);
            }
            let bytes = signature.to_bytes();
            let mut r = [0_u8; 32];
            let mut s = [0_u8; 32];
            r.copy_from_slice(&bytes[..32]);
            s.copy_from_slice(&bytes[32..]);
            let result = EvmTransactionSignature {
                r,
                s,
                y_parity: recovery_id.is_y_odd(),
            };
            result.verify(approval.signing_digest(), self.account.as_str())?;
            Ok(result)
        })
    }

    fn validate_signing(
        &self,
        account: &str,
        transaction_type: ApprovalTransactionType,
        permit: &VaultSigningPermit,
    ) -> Result<(), EvmSigningError> {
        if permit.key_id() != &self.vault_id {
            return Err(EvmSigningError::AuthorizationForDifferentVault);
        }
        if account != self.account.as_str() {
            return Err(EvmSigningError::ApprovalForDifferentAccount);
        }
        if !matches!(
            transaction_type,
            ApprovalTransactionType::Eip1559 {
                transaction_type: 2
            } | ApprovalTransactionType::Eip712
        ) {
            return Err(EvmSigningError::UnsupportedTransactionType);
        }

        Ok(())
    }
}

/// Trusted host observation made after fresh native authentication completes.
///
/// The binding must be freshly reconstructed from the retained signing payload,
/// selected wallet account and simulation evidence. It contains no secret or
/// signing authority.
pub struct EvmSigningObservation<'a> {
    binding: ApprovalBindingInput<'a>,
    now_unix: u64,
    now_monotonic: Instant,
    application_is_active: bool,
}

impl<'a> EvmSigningObservation<'a> {
    /// Creates the exact post-authorization observation used by the signer.
    #[must_use]
    pub const fn new(
        binding: ApprovalBindingInput<'a>,
        now_unix: u64,
        now_monotonic: Instant,
        application_is_active: bool,
    ) -> Self {
        Self {
            binding,
            now_unix,
            now_monotonic,
            application_is_active,
        }
    }
}

/// Host-only recoverable secp256k1 signature components for an EVM transaction.
///
/// This value intentionally has no serialization implementation. The trusted
/// host may hand it directly to transaction assembly, never to generic IPC.
pub struct EvmTransactionSignature {
    r: [u8; 32],
    s: [u8; 32],
    y_parity: bool,
}

impl EvmTransactionSignature {
    /// Independently recovers the exact approved account and enforces low-s.
    ///
    /// # Errors
    /// Returns a closed signature failure for malformed/noncanonical components
    /// or a different digest/account. This never creates signing authority.
    pub fn verify(&self, digest: &[u8; 32], account: &str) -> Result<(), EvmSigningError> {
        let mut bytes = [0_u8; 64];
        bytes[..32].copy_from_slice(&self.r);
        bytes[32..].copy_from_slice(&self.s);
        let signature =
            Signature::from_slice(&bytes).map_err(|_| EvmSigningError::SignatureFailed)?;
        if signature.normalize_s() != signature {
            return Err(EvmSigningError::SignatureFailed);
        }
        let key = VerifyingKey::recover_from_prehash(
            digest,
            &signature,
            RecoveryId::new(self.y_parity, false),
        )
        .map_err(|_| EvmSigningError::SignatureFailed)?;
        let address = from_uncompressed_public_key(key.to_sec1_point(false).as_bytes())
            .ok_or(EvmSigningError::SignatureFailed)?;
        if address.as_str() != account {
            return Err(EvmSigningError::SignatureFailed);
        }
        Ok(())
    }
    /// Returns the canonical 32-byte `r` scalar.
    #[must_use]
    pub const fn r(&self) -> &[u8; 32] {
        &self.r
    }

    /// Returns the canonical low-`s` 32-byte scalar.
    #[must_use]
    pub const fn s(&self) -> &[u8; 32] {
        &self.s
    }

    /// Returns the EIP-1559 signature parity bit.
    #[must_use]
    pub const fn y_parity(&self) -> bool {
        self.y_parity
    }
}

/// Closed failures from the approved EVM signing boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EvmSigningError {
    /// Fresh operating-system user presence was denied or biometric use locked out.
    AuthorizationDenied,
    /// The user cancelled the operating-system authorization prompt.
    AuthorizationCancelled,
    /// Fresh local authentication was unavailable.
    AuthorizationUnavailable,
    /// The app was no longer active when authorization completed.
    ApplicationBackgrounded,
    /// The exact approval changed, expired or otherwise failed verification.
    Approval(ApprovalCommitmentError),
    /// The vault locked or changed while authorization was in progress.
    VaultSession(VaultSessionError),
    /// Fresh authorization belongs to a different vault.
    AuthorizationForDifferentVault,
    /// The approval names a different public account than the private key.
    ApprovalForDifferentAccount,
    /// The approval names a signing domain outside supported EIP-1559 or EIP-712.
    UnsupportedTransactionType,
    /// The private scalar is invalid.
    InvalidPrivateKey,
    /// Recovery-seed child derivation failed.
    KeyDerivationFailed,
    /// Public account derivation failed.
    AddressDerivationFailed,
    /// Deterministic recoverable ECDSA signing failed.
    SignatureFailed,
}

impl fmt::Display for EvmSigningError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::AuthorizationDenied => "fresh owner authorization was denied",
            Self::AuthorizationCancelled => "fresh owner authorization was cancelled",
            Self::AuthorizationUnavailable => "fresh owner authorization is unavailable",
            Self::ApplicationBackgrounded => "application left the trusted foreground session",
            Self::Approval(_) => "exact transaction approval is no longer valid",
            Self::VaultSession(_) => "wallet session is no longer authorized",
            Self::AuthorizationForDifferentVault => {
                "fresh authorization belongs to a different vault"
            }
            Self::ApprovalForDifferentAccount => {
                "exact approval belongs to a different signing account"
            }
            Self::UnsupportedTransactionType => "approval transaction type is unsupported",
            Self::InvalidPrivateKey => "wallet private key is invalid",
            Self::KeyDerivationFailed => "wallet account key derivation failed",
            Self::AddressDerivationFailed => "wallet account address derivation failed",
            Self::SignatureFailed => "EVM signature generation failed",
        })
    }
}

impl EvmSigningError {
    /// Returns a bounded content-free code suitable for local diagnostics.
    #[must_use]
    pub const fn diagnostic_code(self) -> &'static str {
        match self {
            Self::AuthorizationDenied => "authorization_denied",
            Self::AuthorizationCancelled => "authorization_cancelled",
            Self::AuthorizationUnavailable => "authorization_unavailable",
            Self::ApplicationBackgrounded => "application_backgrounded",
            Self::Approval(_) => "approval_invalid",
            Self::VaultSession(_) => "vault_session_invalid",
            Self::AuthorizationForDifferentVault => "authorization_for_different_vault",
            Self::ApprovalForDifferentAccount => "approval_for_different_account",
            Self::UnsupportedTransactionType => "unsupported_transaction_type",
            Self::InvalidPrivateKey => "invalid_private_key",
            Self::KeyDerivationFailed => "key_derivation_failed",
            Self::AddressDerivationFailed => "address_derivation_failed",
            Self::SignatureFailed => "signature_failed",
        }
    }
}

impl Error for EvmSigningError {}

impl From<ApprovalCommitmentError> for EvmSigningError {
    fn from(error: ApprovalCommitmentError) -> Self {
        Self::Approval(error)
    }
}

impl From<VaultSessionError> for EvmSigningError {
    fn from(error: VaultSessionError) -> Self {
        Self::VaultSession(error)
    }
}

pub(crate) trait SigningAuthorizer {
    fn authorize(
        &self,
        key_id: &VaultKeyId,
    ) -> Result<VaultSigningAuthorization, VaultKeyStoreError>;
}

#[cfg(target_os = "macos")]
impl SigningAuthorizer for MacOsVaultKeyStore {
    fn authorize(
        &self,
        key_id: &VaultKeyId,
    ) -> Result<VaultSigningAuthorization, VaultKeyStoreError> {
        self.load_for_signing(key_id)
    }
}

pub(crate) fn authorize_and_sign<'binding>(
    key: &EvmSigningKey,
    commitment: &ApprovalCommitment,
    session: &mut VaultSession,
    authorizer: &impl SigningAuthorizer,
    consumption_store: &impl ApprovalConsumptionStore,
    attempt_started_at_unix: u64,
    observe_after_authorization: impl FnOnce() -> EvmSigningObservation<'binding>,
) -> Result<EvmTransactionSignature, EvmSigningError> {
    if session.status() != VaultSessionStatus::Unlocked {
        return terminal_failure(
            commitment,
            ApprovalInvalidationReason::Failed,
            attempt_started_at_unix,
            consumption_store,
            EvmSigningError::VaultSession(VaultSessionError::Locked),
        );
    }
    let authorization = match authorizer.authorize(session.key_id()) {
        Ok(authorization) => authorization,
        Err(source) => {
            let error = authorization_error(source);
            let reason = if error == EvmSigningError::AuthorizationCancelled {
                ApprovalInvalidationReason::Cancelled
            } else {
                ApprovalInvalidationReason::Failed
            };
            return terminal_failure(
                commitment,
                reason,
                attempt_started_at_unix,
                consumption_store,
                error,
            );
        }
    };
    let observation = observe_after_authorization();
    if !observation.application_is_active {
        return terminal_failure(
            commitment,
            ApprovalInvalidationReason::Cancelled,
            observation.now_unix,
            consumption_store,
            EvmSigningError::ApplicationBackgrounded,
        );
    }
    let verified: VerifiedApproval<'_> =
        match commitment.verify(observation.binding, observation.now_unix) {
            Ok(verified) => verified,
            Err(source) => {
                let reason = if source == ApprovalCommitmentError::Expired {
                    ApprovalInvalidationReason::Expired
                } else {
                    ApprovalInvalidationReason::Superseded
                };
                return terminal_failure(
                    commitment,
                    reason,
                    observation.now_unix,
                    consumption_store,
                    EvmSigningError::Approval(source),
                );
            }
        };
    let permit = match session.authorize_signing(authorization, observation.now_monotonic) {
        Ok(permit) => permit,
        Err(source) => {
            return terminal_failure(
                commitment,
                ApprovalInvalidationReason::Failed,
                observation.now_unix,
                consumption_store,
                EvmSigningError::VaultSession(source),
            );
        }
    };
    if let Err(error) =
        key.validate_signing(verified.account(), verified.transaction_type(), &permit)
    {
        return terminal_failure(
            commitment,
            ApprovalInvalidationReason::Failed,
            observation.now_unix,
            consumption_store,
            error,
        );
    }
    let consumed =
        commitment.consume(observation.binding, observation.now_unix, consumption_store)?;
    key.sign(&consumed, permit)
}

fn terminal_failure<T>(
    commitment: &ApprovalCommitment,
    reason: ApprovalInvalidationReason,
    now_unix: u64,
    consumption_store: &impl ApprovalConsumptionStore,
    error: EvmSigningError,
) -> Result<T, EvmSigningError> {
    commitment.invalidate(reason, now_unix, consumption_store)?;
    Err(error)
}

/// Obtains fresh macOS Keychain user presence and signs one exact approval.
///
/// macOS may satisfy the protected Keychain item's `USER_PRESENCE` policy with
/// Touch ID or the device credential. No model- or frontend-selected fallback
/// exists. The host callback runs only after the OS prompt completes and must
/// freshly observe application focus, time and every TX-004 binding field.
///
/// # Errors
///
/// Denial, cancellation, unavailable authorization, backgrounding, expiry,
/// mutation, vault locking, account mismatch and signing failure all return a
/// closed error without a signature.
#[cfg(target_os = "macos")]
pub fn sign_evm_with_mac_os_user_presence<'binding>(
    key: &EvmSigningKey,
    commitment: &ApprovalCommitment,
    session: &mut VaultSession,
    key_store: &MacOsVaultKeyStore,
    consumption_store: &impl ApprovalConsumptionStore,
    attempt_started_at_unix: u64,
    observe_after_authorization: impl FnOnce() -> EvmSigningObservation<'binding>,
) -> Result<EvmTransactionSignature, EvmSigningError> {
    authorize_and_sign(
        key,
        commitment,
        session,
        key_store,
        consumption_store,
        attempt_started_at_unix,
        observe_after_authorization,
    )
}

fn authorization_error(error: VaultKeyStoreError) -> EvmSigningError {
    match error {
        VaultKeyStoreError::AuthorizationDenied => EvmSigningError::AuthorizationDenied,
        VaultKeyStoreError::AuthorizationCancelled => EvmSigningError::AuthorizationCancelled,
        VaultKeyStoreError::InvalidKeyId
        | VaultKeyStoreError::RandomnessUnavailable
        | VaultKeyStoreError::NotFound
        | VaultKeyStoreError::AuthorizationUnavailable
        | VaultKeyStoreError::SigningConfigurationUnavailable
        | VaultKeyStoreError::InvalidStoredKey
        | VaultKeyStoreError::PlatformFailure => EvmSigningError::AuthorizationUnavailable,
    }
}

#[cfg(test)]
mod tests;
