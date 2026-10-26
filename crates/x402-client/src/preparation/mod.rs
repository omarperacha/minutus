//! Preparation-only x402 EIP-3009 authorization boundary.

mod decode;
mod eip712;
mod types;

use crate::{AdmittedPaymentRequirement, X402_POLICY_REVISION, policy::revalidate_prepared_policy};

pub use types::{
    ActiveX402WalletAccount, PreparedX402Authorization, X402AuthorizationFacts, X402HttpMethod,
    X402PreparationError,
};

use self::{
    decode::decode_and_verify_authorization,
    eip712::{canonical_typed_data, eip712_signing_digest},
    types::PreparedFields,
};

/// Revision of the canonical x402 authorization preparation contract.
pub const X402_PREPARATION_REVISION: u32 = 5;
/// Minimum remaining challenge lifetime accepted for a new approval candidate.
pub const MIN_X402_PREPARATION_LIFETIME_SECONDS: u64 = 15;
/// Client maximum lifetime of a signed authorisation, independently of merchant limits.
pub const MAX_X402_AUTHORIZATION_LIFETIME_SECONDS: u64 = 120;

const NONCE_BYTES: usize = 32;

/// Prepares one exact EIP-3009 authorization without signing or performing I/O.
///
/// `request_identity_sha256` must be the host-owned digest of the complete
/// original request contract, including its method and canonical URL. The
/// method is also retained explicitly for trusted presentation and comparison.
/// The caller must supply host-validated public facts for the currently
/// activated wallet; this crate deliberately has no access to wallet state or
/// signing authority. `None` represents an unavailable or locked selection.
///
/// # Errors
///
/// Returns a closed error when policy evidence is stale, the request is expired
/// or too close to expiry, the public wallet address is invalid, secure nonce
/// generation fails, or independent decoding disagrees with the candidate.
pub fn prepare_x402_payment_authorization(
    requirement: &AdmittedPaymentRequirement,
    method: X402HttpMethod,
    request_identity_sha256: [u8; 32],
    activated_account: Option<&ActiveX402WalletAccount>,
    now_unix_seconds: u64,
) -> Result<PreparedX402Authorization, X402PreparationError> {
    let mut nonce = [0_u8; NONCE_BYTES];
    getrandom::fill(&mut nonce).map_err(|_| X402PreparationError::RandomnessUnavailable)?;
    prepare_with_nonce(
        requirement,
        method,
        request_identity_sha256,
        activated_account,
        now_unix_seconds,
        nonce,
    )
}

fn prepare_with_nonce(
    requirement: &AdmittedPaymentRequirement,
    method: X402HttpMethod,
    request_identity_sha256: [u8; 32],
    activated_account: Option<&ActiveX402WalletAccount>,
    now_unix_seconds: u64,
    nonce: [u8; NONCE_BYTES],
) -> Result<PreparedX402Authorization, X402PreparationError> {
    if requirement.policy_revision() != X402_POLICY_REVISION {
        return Err(X402PreparationError::StalePolicy);
    }
    if request_identity_sha256 == [0; 32] {
        return Err(X402PreparationError::InvalidRequestIdentity);
    }
    if requirement.is_expired_at(now_unix_seconds) {
        return Err(X402PreparationError::ExpiredRequirement);
    }
    let remaining = requirement
        .expires_at_unix_seconds()
        .checked_sub(now_unix_seconds)
        .ok_or(X402PreparationError::ExpiredRequirement)?;
    if remaining < MIN_X402_PREPARATION_LIFETIME_SECONDS {
        return Err(X402PreparationError::ExpiryTooNear);
    }
    if nonce == [0; NONCE_BYTES] {
        return Err(X402PreparationError::InvalidNonce);
    }

    let account = activated_account.ok_or(X402PreparationError::WalletUnavailable)?;
    if account.network() != requirement.network() {
        return Err(X402PreparationError::WalletNetworkMismatch);
    }
    let asset = requirement
        .asset()
        .token_contract()
        .ok_or(X402PreparationError::UnsupportedAsset)?;
    let fields = PreparedFields {
        account: account.address(),
        recipient: requirement.recipient(),
        amount: requirement.amount(),
        valid_after: 0,
        valid_before: requirement
            .expires_at_unix_seconds()
            .min(now_unix_seconds.saturating_add(MAX_X402_AUTHORIZATION_LIFETIME_SECONDS)),
        nonce,
        chain_id: requirement.network().descriptor().chain_id(),
        asset,
    };
    let typed_data = canonical_typed_data(&fields)?;
    let decoded = decode_and_verify_authorization(&typed_data, &fields)?;
    let signing_digest = eip712_signing_digest(&fields)?;

    let candidate = PreparedX402Authorization::new(
        requirement,
        method,
        request_identity_sha256,
        typed_data,
        signing_digest,
        decoded,
    );
    crate::payment::validate_payment_payload_size(&candidate).map_err(|error| match error {
        crate::X402PaymentError::HeaderLimit => X402PreparationError::PaymentHeaderLimit,
        _ => X402PreparationError::EncodingFailed,
    })?;
    Ok(candidate)
}

pub(crate) fn revalidate_prepared_authorization(
    candidate: &PreparedX402Authorization,
) -> Result<(), X402PreparationError> {
    let decoded = candidate.decoded();
    let asset = candidate
        .asset()
        .token_contract()
        .ok_or(X402PreparationError::UnsupportedAsset)?;
    let fields = PreparedFields {
        account: decoded.account(),
        recipient: decoded.recipient(),
        amount: decoded.amount(),
        valid_after: decoded.valid_after(),
        valid_before: decoded.valid_before(),
        nonce: *decoded.nonce(),
        chain_id: decoded.chain_id(),
        asset,
    };
    let reparsed = decode_and_verify_authorization(candidate.canonical_typed_data(), &fields)?;
    let signing_digest = eip712_signing_digest(&fields)?;
    if reparsed != *decoded || signing_digest != *candidate.signing_digest() {
        return Err(X402PreparationError::CandidateMismatch);
    }
    Ok(())
}

/// Revalidates an immutable candidate against current policy and trusted time.
///
/// This check is intended for the signing boundary. It independently decodes
/// the typed data again and rejects stale revisions, changed canonical payloads,
/// disallowed network/asset/resource policy, and expiry.
///
/// # Errors
///
/// Returns a closed preparation error unless every retained candidate fact is
/// still eligible for signing.
pub fn revalidate_prepared_x402_authorization(
    candidate: &PreparedX402Authorization,
    now_unix_seconds: u64,
) -> Result<(), X402PreparationError> {
    if candidate.preparation_revision() != X402_PREPARATION_REVISION
        || candidate.policy_revision() != X402_POLICY_REVISION
    {
        return Err(X402PreparationError::StalePolicy);
    }
    if candidate
        .decoded()
        .valid_before()
        .saturating_sub(now_unix_seconds)
        > MAX_X402_AUTHORIZATION_LIFETIME_SECONDS
    {
        return Err(X402PreparationError::CandidateMismatch);
    }
    revalidate_prepared_policy(
        candidate.resource(),
        candidate.origin_policy(),
        candidate.network(),
        candidate.asset(),
        candidate.payment_extensions(),
        candidate.decoded().valid_before(),
        now_unix_seconds,
    )
    .map_err(|error| match error {
        crate::X402PolicyError::ExpiredRequirement => X402PreparationError::ExpiredRequirement,
        _ => X402PreparationError::StalePolicy,
    })?;
    revalidate_prepared_authorization(candidate)
}

#[cfg(test)]
mod tests;
