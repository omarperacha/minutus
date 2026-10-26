//! Exact, process-local transaction approval commitments with durable single use.

use std::fmt;

use sha3::{Digest as _, Keccak256};

/// Current trusted transaction approval policy revision.
pub const APPROVAL_POLICY_REVISION: u32 = 2;
/// Maximum lifetime of one native approval commitment.
pub const MAX_APPROVAL_LIFETIME_SECONDS: u64 = 5 * 60;

const MAX_SESSION_ID_BYTES: usize = 128;
const COMMITMENT_DOMAIN: &[u8] = b"zeta.transaction-approval.v2";
const APPROVAL_ID_BYTES: usize = 32;

/// Closed signing domain bound by an approval commitment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApprovalTransactionType {
    /// EIP-2718 typed EIP-1559 transaction envelope.
    Eip1559 {
        /// Exact typed-envelope byte, currently `2`.
        transaction_type: u8,
    },
    /// EIP-712 structured-data signing domain.
    Eip712,
}

/// Trusted public facts required to issue or verify one exact commitment.
///
/// Digests bind the exact signing bytes, independently decoded display facts
/// and exact simulation evidence. This value contains no approval authority.
#[derive(Clone, Copy)]
pub struct ApprovalBindingInput<'a> {
    /// Keccak-256 digest signed by the EVM account.
    pub signing_digest: [u8; 32],
    /// EIP-155 chain identifier.
    pub chain_id: u64,
    /// Exact public signing account.
    pub account: &'a str,
    /// Host-created request session identity.
    pub session_id: &'a str,
    /// Absolute Unix expiry in seconds.
    pub expires_at_unix: u64,
    /// Typed transaction or structured-data domain.
    pub transaction_type: ApprovalTransactionType,
    /// Digest of the independently decoded security facts shown to the user.
    pub decoder_digest: [u8; 32],
    /// Digest of the exact simulation evidence shown to the user.
    pub simulation_digest: [u8; 32],
}

#[derive(Clone, Eq, PartialEq)]
struct ApprovalBinding {
    approval_id: [u8; APPROVAL_ID_BYTES],
    signing_digest: [u8; 32],
    chain_id: u64,
    account: String,
    session_id: String,
    expires_at_unix: u64,
    transaction_type: ApprovalTransactionType,
    decoder_digest: [u8; 32],
    simulation_digest: [u8; 32],
    policy_revision: u32,
}

/// Opaque process-local proof that the native user approved one exact binding.
///
/// The type deliberately implements neither cloning, debugging nor
/// serialization. Restart drops it, and it contains no private key or signing
/// callback.
pub struct ApprovalCommitment {
    binding: ApprovalBinding,
    commitment_digest: [u8; 32],
}

impl ApprovalCommitment {
    /// Issues a commitment from facts prepared by trusted native host code.
    ///
    /// This function must be called only after the trusted native approval
    /// surface returns its explicit Approved decision.
    ///
    /// # Errors
    ///
    /// Returns a closed validation error for a malformed account, session,
    /// transaction type or expiry outside the fixed lifetime.
    pub fn issue(
        input: ApprovalBindingInput<'_>,
        now_unix: u64,
    ) -> Result<Self, ApprovalCommitmentError> {
        let mut approval_id = [0_u8; APPROVAL_ID_BYTES];
        getrandom::fill(&mut approval_id)
            .map_err(|_| ApprovalCommitmentError::RandomnessUnavailable)?;
        let binding = ApprovalBinding::validated(approval_id, input, now_unix)?;
        let commitment_digest = commitment_digest(&binding);
        Ok(Self {
            binding,
            commitment_digest,
        })
    }

    /// Recomputes and verifies every bound fact immediately before signing.
    ///
    /// Digest comparisons are constant-time even though the committed facts are
    /// public. The returned value is lifetime-bound to this live commitment and
    /// cannot be serialized into signing authority.
    ///
    /// # Errors
    ///
    /// Returns a field-specific mismatch or expiry error. Any change requires a
    /// new decode, simulation and native approval.
    pub fn verify<'a>(
        &'a self,
        input: ApprovalBindingInput<'_>,
        now_unix: u64,
    ) -> Result<VerifiedApproval<'a>, ApprovalCommitmentError> {
        if now_unix >= self.binding.expires_at_unix {
            return Err(ApprovalCommitmentError::Expired);
        }
        let expected = ApprovalBinding::validated(self.binding.approval_id, input, now_unix)?;
        compare_binding(&self.binding, &expected)?;
        let recomputed = commitment_digest(&expected);
        if !constant_time_eq(&self.commitment_digest, &recomputed) {
            return Err(ApprovalCommitmentError::CommitmentMismatch);
        }
        Ok(VerifiedApproval { commitment: self })
    }

    /// Atomically consumes this exact approval in durable trusted storage.
    ///
    /// Verification happens before the durable claim. The returned token is the
    /// only approval capability accepted by the wallet signing boundary and
    /// cannot be cloned, serialized or reconstructed after restart.
    ///
    /// # Errors
    ///
    /// Returns a closed error when the binding is stale or changed, when the
    /// approval was already finalized, or when durable storage cannot claim it.
    pub fn consume<'a>(
        &'a self,
        input: ApprovalBindingInput<'_>,
        now_unix: u64,
        store: &impl ApprovalConsumptionStore,
    ) -> Result<ConsumedApproval<'a>, ApprovalCommitmentError> {
        let verified = self.verify(input, now_unix)?;
        let record = ApprovalConsumptionRecord::new(self, now_unix);
        match store.finalize(&record, ApprovalFinalization::Consumed) {
            ApprovalConsumptionOutcome::Finalized => Ok(ConsumedApproval { verified }),
            ApprovalConsumptionOutcome::AlreadyConsumed => {
                Err(ApprovalCommitmentError::AlreadyConsumed)
            }
            ApprovalConsumptionOutcome::Invalidated => Err(ApprovalCommitmentError::Invalidated),
            ApprovalConsumptionOutcome::IdentifierCollision => {
                Err(ApprovalCommitmentError::IdentifierCollision)
            }
            ApprovalConsumptionOutcome::Unavailable => {
                Err(ApprovalCommitmentError::ConsumptionStoreUnavailable)
            }
        }
    }

    /// Durably makes this approval unusable for a bounded terminal reason.
    ///
    /// This is used when an approved request is cancelled, fails or is
    /// superseded. Once invalidated, the identifier can never be consumed.
    ///
    /// # Errors
    ///
    /// Returns a closed error if the approval was already consumed, its
    /// identifier collides with different binding data, or storage is down.
    pub fn invalidate(
        &self,
        reason: ApprovalInvalidationReason,
        now_unix: u64,
        store: &impl ApprovalConsumptionStore,
    ) -> Result<(), ApprovalCommitmentError> {
        let record = ApprovalConsumptionRecord::new(self, now_unix);
        match store.finalize(&record, ApprovalFinalization::Invalidated(reason)) {
            ApprovalConsumptionOutcome::Finalized | ApprovalConsumptionOutcome::Invalidated => {
                Ok(())
            }
            ApprovalConsumptionOutcome::AlreadyConsumed => {
                Err(ApprovalCommitmentError::AlreadyConsumed)
            }
            ApprovalConsumptionOutcome::IdentifierCollision => {
                Err(ApprovalCommitmentError::IdentifierCollision)
            }
            ApprovalConsumptionOutcome::Unavailable => {
                Err(ApprovalCommitmentError::ConsumptionStoreUnavailable)
            }
        }
    }

    /// Returns whether the process-local commitment has reached its absolute expiry.
    #[must_use]
    pub const fn is_expired(&self, now_unix: u64) -> bool {
        now_unix >= self.binding.expires_at_unix
    }
}

impl ApprovalBinding {
    fn validated(
        approval_id: [u8; APPROVAL_ID_BYTES],
        input: ApprovalBindingInput<'_>,
        now_unix: u64,
    ) -> Result<Self, ApprovalCommitmentError> {
        if !is_evm_account(input.account) {
            return Err(ApprovalCommitmentError::InvalidAccount);
        }
        if input.chain_id == 0 {
            return Err(ApprovalCommitmentError::InvalidChain);
        }
        if input.session_id.is_empty()
            || input.session_id.len() > MAX_SESSION_ID_BYTES
            || !input
                .session_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(ApprovalCommitmentError::InvalidSession);
        }
        if input.expires_at_unix <= now_unix
            || input.expires_at_unix - now_unix > MAX_APPROVAL_LIFETIME_SECONDS
        {
            return Err(ApprovalCommitmentError::InvalidExpiry);
        }
        if !matches!(
            input.transaction_type,
            ApprovalTransactionType::Eip1559 {
                transaction_type: 2
            } | ApprovalTransactionType::Eip712
        ) {
            return Err(ApprovalCommitmentError::UnsupportedTransactionType);
        }
        Ok(Self {
            approval_id,
            signing_digest: input.signing_digest,
            chain_id: input.chain_id,
            account: input.account.to_owned(),
            session_id: input.session_id.to_owned(),
            expires_at_unix: input.expires_at_unix,
            transaction_type: input.transaction_type,
            decoder_digest: input.decoder_digest,
            simulation_digest: input.simulation_digest,
            policy_revision: APPROVAL_POLICY_REVISION,
        })
    }
}

/// Bounded terminal reason that prevents an issued approval from being used.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApprovalInvalidationReason {
    /// The user or trusted OS authorization flow cancelled the request.
    Cancelled,
    /// The request expired before signature release.
    Expired,
    /// A trusted prerequisite failed after approval.
    Failed,
    /// A newer request replaced this approval.
    Superseded,
}

impl ApprovalInvalidationReason {
    /// Returns the stable bounded storage code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::Expired => "expired",
            Self::Failed => "failed",
            Self::Superseded => "superseded",
        }
    }
}

/// Requested durable terminal state for one exact approval.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApprovalFinalization {
    /// Claim the approval immediately before signature creation.
    Consumed,
    /// Prevent signing for one bounded terminal reason.
    Invalidated(ApprovalInvalidationReason),
}

impl ApprovalFinalization {
    /// Returns the stable bounded disposition code.
    #[must_use]
    pub const fn disposition(self) -> &'static str {
        match self {
            Self::Consumed => "consumed",
            Self::Invalidated(_) => "invalidated",
        }
    }

    /// Returns the bounded invalidation code, when applicable.
    #[must_use]
    pub const fn reason(self) -> Option<&'static str> {
        match self {
            Self::Consumed => None,
            Self::Invalidated(reason) => Some(reason.as_str()),
        }
    }
}

/// Non-secret exact metadata durably finalized for replay prevention.
///
/// All strings and digests are produced by trusted policy code. The value
/// contains no approval capability, raw payload, signature or secret material.
pub struct ApprovalConsumptionRecord<'a> {
    commitment: &'a ApprovalCommitment,
    finalized_at_unix: u64,
}

impl ApprovalConsumptionRecord<'_> {
    fn new(
        commitment: &ApprovalCommitment,
        finalized_at_unix: u64,
    ) -> ApprovalConsumptionRecord<'_> {
        ApprovalConsumptionRecord {
            commitment,
            finalized_at_unix,
        }
    }

    /// Returns the random single-use identifier as lowercase hexadecimal.
    #[must_use]
    pub fn approval_id_hex(&self) -> String {
        hex(&self.commitment.binding.approval_id)
    }

    /// Returns the digest binding the identifier and all approval facts.
    #[must_use]
    pub fn commitment_digest_hex(&self) -> String {
        hex(&self.commitment.commitment_digest)
    }

    /// Returns the public EVM signing digest as lowercase hexadecimal.
    #[must_use]
    pub fn signing_digest_hex(&self) -> String {
        hex(&self.commitment.binding.signing_digest)
    }

    /// Returns the exact chain ID.
    #[must_use]
    pub const fn chain_id(&self) -> u64 {
        self.commitment.binding.chain_id
    }

    /// Returns the exact public account.
    #[must_use]
    pub fn account(&self) -> &str {
        &self.commitment.binding.account
    }

    /// Returns the exact host session identifier.
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.commitment.binding.session_id
    }

    /// Returns the absolute approval expiry.
    #[must_use]
    pub const fn expires_at_unix(&self) -> u64 {
        self.commitment.binding.expires_at_unix
    }

    /// Returns the closed transaction type code.
    #[must_use]
    pub const fn transaction_type(&self) -> &'static str {
        match self.commitment.binding.transaction_type {
            ApprovalTransactionType::Eip1559 {
                transaction_type: 2,
            } => "eip1559:2",
            ApprovalTransactionType::Eip1559 { .. } => "unsupported",
            ApprovalTransactionType::Eip712 => "eip712",
        }
    }

    /// Returns the approval policy revision.
    #[must_use]
    pub const fn policy_revision(&self) -> u32 {
        self.commitment.binding.policy_revision
    }

    /// Returns the trusted finalization timestamp.
    #[must_use]
    pub const fn finalized_at_unix(&self) -> u64 {
        self.finalized_at_unix
    }
}

/// Closed outcome of one atomic durable finalization attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApprovalConsumptionOutcome {
    /// This call durably established the requested terminal state.
    Finalized,
    /// The exact approval was consumed by an earlier call or process.
    AlreadyConsumed,
    /// The exact approval was invalidated before consumption.
    Invalidated,
    /// The random identifier already names different binding data.
    IdentifierCollision,
    /// Durable storage could not establish a terminal result.
    Unavailable,
}

/// Trusted durable boundary used to finalize approval identifiers atomically.
pub trait ApprovalConsumptionStore {
    /// Atomically inserts the terminal state or reports its existing state.
    fn finalize(
        &self,
        record: &ApprovalConsumptionRecord<'_>,
        finalization: ApprovalFinalization,
    ) -> ApprovalConsumptionOutcome;
}

/// Ephemeral proof that a live commitment exactly matched fresh signing facts.
///
/// This type implements neither cloning, debugging nor serialization. The
/// wallet uses it only for pre-consumption identity checks; signature creation
/// requires the distinct durable `ConsumedApproval` capability.
pub struct VerifiedApproval<'a> {
    commitment: &'a ApprovalCommitment,
}

/// Single-use proof that exact approval was durably consumed.
///
/// This type cannot be cloned, serialized or reconstructed. Wallet signing
/// accepts this type rather than a merely verified approval.
pub struct ConsumedApproval<'a> {
    verified: VerifiedApproval<'a>,
}

impl ConsumedApproval<'_> {
    /// Returns the exact Ethereum digest authorized for this one signing call.
    #[must_use]
    pub const fn signing_digest(&self) -> &[u8; 32] {
        self.verified.signing_digest()
    }

    /// Returns the exact public account authorized for signing.
    #[must_use]
    pub fn account(&self) -> &str {
        self.verified.account()
    }

    /// Returns the closed transaction domain authorized for signing.
    #[must_use]
    pub const fn transaction_type(&self) -> ApprovalTransactionType {
        self.verified.transaction_type()
    }
}

impl VerifiedApproval<'_> {
    /// Returns the exact Ethereum digest authorized for signing.
    #[must_use]
    pub const fn signing_digest(&self) -> &[u8; 32] {
        &self.commitment.binding.signing_digest
    }

    /// Returns the exact public account authorized for signing.
    #[must_use]
    pub fn account(&self) -> &str {
        &self.commitment.binding.account
    }

    /// Returns the closed transaction domain authorized for signing.
    #[must_use]
    pub const fn transaction_type(&self) -> ApprovalTransactionType {
        self.commitment.binding.transaction_type
    }
}

/// Closed commitment construction and verification failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApprovalCommitmentError {
    /// Secure random generation failed while creating the single-use identity.
    RandomnessUnavailable,
    /// The public account is not an exact 20-byte EVM address.
    InvalidAccount,
    /// Chain ID zero is not a valid EVM approval domain.
    InvalidChain,
    /// The host session identifier is empty, oversized or malformed.
    InvalidSession,
    /// Expiry is not future-bound within the fixed maximum lifetime.
    InvalidExpiry,
    /// The requested transaction type is not supported by the policy revision.
    UnsupportedTransactionType,
    /// The approval reached its absolute expiry.
    Expired,
    /// Exact signing bytes changed.
    SigningDigestMismatch,
    /// Chain identity changed.
    ChainMismatch,
    /// Public signing account changed.
    AccountMismatch,
    /// Host request session changed.
    SessionMismatch,
    /// Absolute expiry changed.
    ExpiryMismatch,
    /// Typed transaction or signing domain changed.
    TransactionTypeMismatch,
    /// Independently decoded display facts changed.
    DecoderMismatch,
    /// Exact simulation evidence changed.
    SimulationMismatch,
    /// Trusted approval policy revision changed.
    PolicyMismatch,
    /// The canonical commitment could not be reproduced from the supplied facts.
    CommitmentMismatch,
    /// This exact approval already authorized a signing attempt.
    AlreadyConsumed,
    /// This exact approval was terminally invalidated.
    Invalidated,
    /// The single-use identifier is already bound to different approval data.
    IdentifierCollision,
    /// Durable replay-prevention storage could not establish consumption.
    ConsumptionStoreUnavailable,
}

impl fmt::Display for ApprovalCommitmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::RandomnessUnavailable => "secure approval identity generation is unavailable",
            Self::InvalidAccount => "approval account is malformed",
            Self::InvalidChain => "approval chain ID is invalid",
            Self::InvalidSession => "approval session is malformed",
            Self::InvalidExpiry => "approval expiry is outside the permitted lifetime",
            Self::UnsupportedTransactionType => "approval transaction type is unsupported",
            Self::Expired => "approval commitment has expired",
            Self::SigningDigestMismatch => "signing payload digest changed after approval",
            Self::ChainMismatch => "transaction chain changed after approval",
            Self::AccountMismatch => "signing account changed after approval",
            Self::SessionMismatch => "transaction session changed after approval",
            Self::ExpiryMismatch => "approval expiry changed after approval",
            Self::TransactionTypeMismatch => "transaction type changed after approval",
            Self::DecoderMismatch => "decoded approval facts changed after approval",
            Self::SimulationMismatch => "simulation evidence changed after approval",
            Self::PolicyMismatch => "approval policy revision changed after approval",
            Self::CommitmentMismatch => "approval commitment digest did not reproduce exactly",
            Self::AlreadyConsumed => "approval commitment was already consumed",
            Self::Invalidated => "approval commitment was terminally invalidated",
            Self::IdentifierCollision => "approval identifier is bound to different data",
            Self::ConsumptionStoreUnavailable => "durable approval consumption is unavailable",
        })
    }
}

impl std::error::Error for ApprovalCommitmentError {}

fn compare_binding(
    approved: &ApprovalBinding,
    expected: &ApprovalBinding,
) -> Result<(), ApprovalCommitmentError> {
    if !constant_time_eq(&approved.signing_digest, &expected.signing_digest) {
        return Err(ApprovalCommitmentError::SigningDigestMismatch);
    }
    if approved.chain_id != expected.chain_id {
        return Err(ApprovalCommitmentError::ChainMismatch);
    }
    if approved.account != expected.account {
        return Err(ApprovalCommitmentError::AccountMismatch);
    }
    if approved.session_id != expected.session_id {
        return Err(ApprovalCommitmentError::SessionMismatch);
    }
    if approved.expires_at_unix != expected.expires_at_unix {
        return Err(ApprovalCommitmentError::ExpiryMismatch);
    }
    if approved.transaction_type != expected.transaction_type {
        return Err(ApprovalCommitmentError::TransactionTypeMismatch);
    }
    if !constant_time_eq(&approved.decoder_digest, &expected.decoder_digest) {
        return Err(ApprovalCommitmentError::DecoderMismatch);
    }
    if !constant_time_eq(&approved.simulation_digest, &expected.simulation_digest) {
        return Err(ApprovalCommitmentError::SimulationMismatch);
    }
    if approved.policy_revision != expected.policy_revision {
        return Err(ApprovalCommitmentError::PolicyMismatch);
    }
    Ok(())
}

fn commitment_digest(binding: &ApprovalBinding) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    hasher.update(COMMITMENT_DOMAIN);
    hasher.update(binding.policy_revision.to_be_bytes());
    hasher.update(binding.approval_id);
    hasher.update(binding.signing_digest);
    hasher.update(binding.chain_id.to_be_bytes());
    frame(&mut hasher, binding.account.as_bytes());
    frame(&mut hasher, binding.session_id.as_bytes());
    hasher.update(binding.expires_at_unix.to_be_bytes());
    match binding.transaction_type {
        ApprovalTransactionType::Eip1559 { transaction_type } => {
            hasher.update([0, transaction_type]);
        }
        ApprovalTransactionType::Eip712 => hasher.update([1, 0]),
    }
    hasher.update(binding.decoder_digest);
    hasher.update(binding.simulation_digest);
    hasher.finalize().into()
}

fn hex(bytes: &[u8]) -> String {
    use fmt::Write as _;

    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn frame(hasher: &mut Keccak256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

fn constant_time_eq(left: &[u8; 32], right: &[u8; 32]) -> bool {
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn is_evm_account(value: &str) -> bool {
    value.len() == 42
        && value.starts_with("0x")
        && value[2..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests;
