use std::fmt::Write as _;

use crate::PreparedX402Authorization;
#[cfg(debug_assertions)]
use crate::X402OriginPolicy;

use super::{
    X402_EXPIRY_WARNING_SECONDS, X402_UNUSUAL_PRICE_ATOMIC_UNITS, X402AgentReason,
    X402ApprovalError, X402ApprovalPresentation, X402ApprovalWarning,
};

pub(super) fn build(
    candidate: &PreparedX402Authorization,
    agent_reason: Option<X402AgentReason>,
    expires_at_unix: u64,
    now_unix: u64,
) -> Result<X402ApprovalPresentation, X402ApprovalError> {
    let decoded = candidate.decoded();
    let asset = candidate.asset();
    let contract = asset
        .token_contract()
        .ok_or(X402ApprovalError::ValidationUnavailable)?;
    let price = format_atomic_amount(decoded.amount(), asset.decimals())?;
    let descriptor = candidate.network().descriptor();
    let network = format!(
        "{} · {} · chain {}",
        descriptor.display_name(),
        descriptor.caip2_id(),
        descriptor.chain_id()
    );
    let mut warnings = Vec::new();
    #[cfg(debug_assertions)]
    if candidate.origin_policy() == X402OriginPolicy::DebugLoopback {
        warnings.push(X402ApprovalWarning::LocalDevelopmentEndpoint);
    }
    if descriptor.is_testnet() {
        warnings.push(X402ApprovalWarning::Testnet);
    }
    // X402-002 has no recipient allow-list. A server-selected pay-to address
    // therefore remains explicitly unrecognized even though it is well-formed.
    warnings.push(X402ApprovalWarning::UnrecognizedRecipient);
    if expires_at_unix.saturating_sub(now_unix) <= X402_EXPIRY_WARNING_SECONDS {
        warnings.push(X402ApprovalWarning::ExpiringAuthorization);
    }
    if decimal_greater_than(decoded.amount(), X402_UNUSUAL_PRICE_ATOMIC_UNITS) {
        warnings.push(X402ApprovalWarning::UnusualPrice);
    }
    warnings.sort_unstable();
    warnings.dedup();

    let domain = candidate.resource().origin().to_owned();
    let resource = candidate.resource().canonical_url().to_owned();
    let price_with_symbol = format!("{price} {}", asset.symbol());
    let title = format!(
        "⚠ {} warning(s) — review exact x402 payment",
        warnings.len()
    );
    let body = render_body(
        candidate,
        &domain,
        &resource,
        &price_with_symbol,
        contract,
        &network,
        expires_at_unix,
        agent_reason.as_ref(),
        &warnings,
    );
    Ok(X402ApprovalPresentation {
        title,
        body,
        domain,
        resource,
        price: price_with_symbol,
        atomic_amount: decoded.amount().to_owned(),
        asset_symbol: asset.symbol().to_owned(),
        asset_contract: contract.to_owned(),
        network,
        payer: decoded.account().to_owned(),
        recipient: decoded.recipient().to_owned(),
        expires_at_unix,
        agent_reason,
        warnings,
    })
}

#[allow(clippy::too_many_arguments)]
fn render_body(
    candidate: &PreparedX402Authorization,
    domain: &str,
    resource: &str,
    price: &str,
    contract: &str,
    network: &str,
    expires_at_unix: u64,
    agent_reason: Option<&X402AgentReason>,
    warnings: &[X402ApprovalWarning],
) -> String {
    let decoded = candidate.decoded();
    let mut body = String::new();
    body.push_str("WARNINGS — REVIEW BEFORE APPROVING\n");
    for warning in warnings {
        let _ = writeln!(body, "• {}", warning.message());
    }
    body.push_str("\nTRUSTED PAYMENT FACTS\n");
    let _ = writeln!(body, "Domain: {domain}");
    let _ = writeln!(body, "Requested resource: {resource}");
    let _ = writeln!(body, "HTTP method: {}", candidate.method().as_str());
    let _ = writeln!(body, "Exact price: {price}");
    let _ = writeln!(body, "Atomic units: {}", decoded.amount());
    let _ = writeln!(body, "Asset contract: {contract}");
    let _ = writeln!(body, "Network: {network}");
    let _ = writeln!(body, "Paying wallet: {}", decoded.account());
    let _ = writeln!(body, "Recipient: {}", decoded.recipient());
    body.push_str(
        "Facilitator: not identified by the challenge; the server may submit this authorization\n",
    );
    let _ = writeln!(
        body,
        "Signed authorisation expires: Unix {}",
        decoded.valid_before()
    );
    let _ = writeln!(body, "Local approval expires: Unix {expires_at_unix}");
    body.push_str(
        "Once sent, the merchant can submit this exact authorisation until its signed expiry.\n",
    );
    let _ = writeln!(
        body,
        "Candidate digest: 0x{}",
        hex(candidate.candidate_digest())
    );
    body.push_str("\nUNTRUSTED AGENT CONTEXT — NOT PAYMENT FACTS\n");
    body.push_str(agent_reason.map_or("No reason was supplied.", X402AgentReason::as_str));
    body
}

fn format_atomic_amount(amount: &str, decimals: u8) -> Result<String, X402ApprovalError> {
    if amount.is_empty() || !amount.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(X402ApprovalError::InvalidAssetAmount);
    }
    let decimals = usize::from(decimals);
    if decimals == 0 {
        return Ok(amount.to_owned());
    }
    if amount.len() <= decimals {
        let zeroes = "0".repeat(decimals - amount.len());
        return Ok(format!("0.{zeroes}{amount}"));
    }
    let split = amount.len() - decimals;
    Ok(format!("{}.{}", &amount[..split], &amount[split..]))
}

fn decimal_greater_than(left: &str, right: &str) -> bool {
    left.len() > right.len() || (left.len() == right.len() && left > right)
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(char::from(DIGITS[usize::from(byte >> 4)]));
        result.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    result
}
