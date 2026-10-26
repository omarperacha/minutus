//! Trusted x402 projection and exact native-approval evidence.

mod presentation;
mod types;

use sha2::{Digest as _, Sha256};

use crate::{PreparedX402Authorization, X402_POLICY_REVISION};

pub use types::{
    PreparedX402Approval, X402AgentReason, X402ApprovalDecision, X402ApprovalError,
    X402ApprovalPresentation, X402ApprovalWarning,
};

/// Maximum lifetime of one x402 native approval review.
pub const X402_APPROVAL_LIFETIME_SECONDS: u64 = 2 * 60;
/// Remaining lifetime at or below which expiry is shown as prominent.
pub const X402_EXPIRY_WARNING_SECONDS: u64 = 30;
/// Development warning threshold: more than 10 USDC in atomic units.
pub const X402_UNUSUAL_PRICE_ATOMIC_UNITS: &str = "10000000";
/// Maximum UTF-8 byte length accepted for untrusted agent context.
pub const MAX_X402_AGENT_REASON_BYTES: usize = 512;

const VALIDATION_EVIDENCE_DOMAIN: &[u8] = b"zeta.x402-approval-validation.v1";

/// Revalidates and projects one prepared authorization for trusted native review.
///
/// `wallet_session_id` is host-created trusted state, not a model or frontend
/// value. The optional reason remains explicitly untrusted context and is never
/// used to derive a payment fact, warning, expiry or approval binding.
///
/// This function performs no signing, approval, wallet access or network I/O.
/// The returned binding input is public evidence only. A native host must issue
/// an Epic 13 commitment only after receiving an explicit `Approved` decision.
///
/// # Errors
///
/// Returns a closed failure when the policy or candidate is stale, independent
/// revalidation is unavailable, the wallet session is malformed, or untrusted
/// context cannot be rendered without confusing the trusted presentation.
pub fn prepare_x402_payment_approval(
    candidate: &PreparedX402Authorization,
    wallet_session_id: &str,
    agent_reason: Option<&str>,
    now_unix_seconds: u64,
) -> Result<PreparedX402Approval, X402ApprovalError> {
    if candidate.policy_revision() != X402_POLICY_REVISION {
        return Err(X402ApprovalError::StalePolicy);
    }
    if candidate.is_expired_at(now_unix_seconds) {
        return Err(X402ApprovalError::Expired);
    }
    validate_session_id(wallet_session_id)?;
    crate::preparation::revalidate_prepared_authorization(candidate)
        .map_err(|_| X402ApprovalError::ValidationUnavailable)?;

    let agent_reason = agent_reason.map(X402AgentReason::new).transpose()?;
    let expires_at_unix = candidate
        .decoded()
        .valid_before()
        .min(now_unix_seconds.saturating_add(X402_APPROVAL_LIFETIME_SECONDS));
    if expires_at_unix <= now_unix_seconds {
        return Err(X402ApprovalError::Expired);
    }
    let validation_digest = validation_evidence_digest(candidate);
    let presentation =
        presentation::build(candidate, agent_reason, expires_at_unix, now_unix_seconds)?;
    Ok(PreparedX402Approval::new(
        candidate.clone(),
        wallet_session_id,
        expires_at_unix,
        validation_digest,
        presentation,
    ))
}

fn validate_session_id(session_id: &str) -> Result<(), X402ApprovalError> {
    if session_id.is_empty()
        || session_id.len() > 128
        || !session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(X402ApprovalError::InvalidWalletSession);
    }
    Ok(())
}

fn validation_evidence_digest(candidate: &PreparedX402Authorization) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(VALIDATION_EVIDENCE_DOMAIN);
    hasher.update(candidate.candidate_digest());
    hasher.update(candidate.policy_revision().to_be_bytes());
    hasher.update(candidate.preparation_revision().to_be_bytes());
    hasher.finalize().into()
}

#[cfg(test)]
mod tests;
