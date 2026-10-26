//! Canonical x402 v2 payment header and settlement-response validation.

use std::{error::Error, fmt};

use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
};
use chain_evm::PublicEvmAddress;
use serde::{Deserialize, Serialize};

use crate::{PreparedX402Authorization, X402_V2, response::UnambiguousJson};

/// Maximum encoded bytes accepted for either x402 payment header.
pub const MAX_X402_PAYMENT_HEADER_BYTES: usize = 16 * 1024;
const MAX_X402_PAYMENT_RESPONSE_BYTES: usize = 8 * 1024;

/// Host-only `PAYMENT-SIGNATURE` value. It has no debug or serialization view.
pub struct X402PaymentSignatureHeader(String);

impl X402PaymentSignatureHeader {
    /// Returns the canonical base64 header value for the one paid retry.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Validated public settlement evidence from `PAYMENT-RESPONSE`.
#[derive(Clone, Eq, PartialEq)]
pub struct ValidatedX402PaymentResponse {
    transaction: String,
    network: String,
    payer: String,
    amount: Option<String>,
}

impl ValidatedX402PaymentResponse {
    /// Onchain settlement transaction hash.
    #[must_use]
    pub fn transaction(&self) -> &str {
        &self.transaction
    }
    /// Exact CAIP-2 network returned by settlement.
    #[must_use]
    pub fn network(&self) -> &str {
        &self.network
    }
    /// Normalized payer address matching the approved account.
    #[must_use]
    pub fn payer(&self) -> &str {
        &self.payer
    }
    /// Optional exact settled amount, when the server supplies it.
    #[must_use]
    pub fn amount(&self) -> Option<&str> {
        self.amount.as_deref()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PaymentPayload<'a> {
    x402_version: u8,
    resource: PaymentResource<'a>,
    accepted: AcceptedPayment<'a>,
    payload: ExactEvmPayload<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    extensions: Option<&'a serde_json::Map<String, serde_json::Value>>,
}

#[derive(Serialize)]
struct PaymentResource<'a> {
    url: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AcceptedPayment<'a> {
    scheme: &'static str,
    network: &'static str,
    amount: &'a str,
    asset: &'static str,
    pay_to: &'a str,
    max_timeout_seconds: u64,
    extra: ExactEvmExtra<'a>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExactEvmExtra<'a> {
    name: &'a str,
    version: &'a str,
    asset_transfer_method: &'static str,
    payment_flow: &'static str,
}

#[derive(Serialize)]
struct ExactEvmPayload<'a> {
    signature: String,
    authorization: Authorization<'a>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Authorization<'a> {
    from: &'a str,
    to: &'a str,
    value: &'a str,
    valid_after: String,
    valid_before: String,
    nonce: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SettlementResponse {
    success: bool,
    error_reason: Option<String>,
    payer: Option<String>,
    transaction: String,
    network: String,
    amount: Option<String>,
    extensions: Option<serde_json::Map<String, serde_json::Value>>,
}

/// Builds the exact v2 EIP-3009 payment header from a recoverable signature.
///
/// # Errors
///
/// Fails closed for an invalid signature scalar or oversized encoding.
pub fn create_x402_payment_signature_header(
    candidate: &PreparedX402Authorization,
    r: &[u8; 32],
    s: &[u8; 32],
    y_parity: bool,
) -> Result<X402PaymentSignatureHeader, X402PaymentError> {
    if *r == [0; 32] || *s == [0; 32] {
        return Err(X402PaymentError::InvalidSignature);
    }
    encode_payment_payload(candidate, signature_hex(r, s, y_parity)).map(X402PaymentSignatureHeader)
}

/// Check the actual serialized payload budget before native approval/signing.
/// Signature hex has a fixed length, so a placeholder measures the same size
/// without acquiring a signature or producing a usable payment header.
pub(crate) fn validate_payment_payload_size(
    candidate: &PreparedX402Authorization,
) -> Result<(), X402PaymentError> {
    encode_payment_payload(candidate, format!("0x{}", "0".repeat(130))).map(|_| ())
}

fn encode_payment_payload(
    candidate: &PreparedX402Authorization,
    signature: String,
) -> Result<String, X402PaymentError> {
    let decoded = candidate.decoded();
    let asset = candidate
        .asset()
        .token_contract()
        .ok_or(X402PaymentError::CandidateMismatch)?;
    let payload = PaymentPayload {
        x402_version: X402_V2,
        extensions: candidate.payment_extensions(),
        resource: PaymentResource {
            url: candidate.resource().canonical_url(),
        },
        accepted: AcceptedPayment {
            scheme: "exact",
            network: candidate.network().descriptor().caip2_id(),
            amount: decoded.amount(),
            asset,
            pay_to: decoded.recipient(),
            max_timeout_seconds: candidate.max_timeout_seconds(),
            extra: ExactEvmExtra {
                name: decoded.domain_name(),
                version: decoded.domain_version(),
                asset_transfer_method: "eip3009",
                payment_flow: "authorization",
            },
        },
        payload: ExactEvmPayload {
            signature,
            authorization: Authorization {
                from: decoded.account(),
                to: decoded.recipient(),
                value: decoded.amount(),
                valid_after: decoded.valid_after().to_string(),
                valid_before: decoded.valid_before().to_string(),
                nonce: hex_prefixed(decoded.nonce()),
            },
        },
    };
    let json = serde_json::to_vec(&payload).map_err(|_| X402PaymentError::EncodingFailed)?;
    let encoded = STANDARD.encode(json);
    if encoded.len() > MAX_X402_PAYMENT_HEADER_BYTES {
        return Err(X402PaymentError::HeaderLimit);
    }
    Ok(encoded)
}

/// Validates exactly one successful v2 `PAYMENT-RESPONSE` header.
///
/// # Errors
///
/// Missing, duplicate, malformed, unsuccessful or candidate-mismatched
/// settlement evidence is rejected and must never be interpreted as purchase
/// success.
pub fn validate_x402_payment_response(
    status: u16,
    headers: &[&[u8]],
    candidate: &PreparedX402Authorization,
) -> Result<ValidatedX402PaymentResponse, X402PaymentError> {
    if status / 100 != 2 {
        return Err(X402PaymentError::UnexpectedStatus);
    }
    let encoded = match headers {
        [value] => *value,
        [] => return Err(X402PaymentError::MissingResponse),
        [_, _, ..] => return Err(X402PaymentError::DuplicateResponse),
    };
    if encoded.is_empty() || encoded.len() > MAX_X402_PAYMENT_HEADER_BYTES {
        return Err(X402PaymentError::HeaderLimit);
    }
    let decoded = STANDARD
        .decode(encoded)
        .or_else(|_| STANDARD_NO_PAD.decode(encoded))
        .map_err(|_| X402PaymentError::MalformedResponse)?;
    if decoded.is_empty() || decoded.len() > MAX_X402_PAYMENT_RESPONSE_BYTES {
        return Err(X402PaymentError::HeaderLimit);
    }
    serde_json::from_slice::<UnambiguousJson>(&decoded)
        .map_err(|_| X402PaymentError::MalformedResponse)?;
    let response: SettlementResponse =
        serde_json::from_slice(&decoded).map_err(|_| X402PaymentError::MalformedResponse)?;
    if !response.success || response.error_reason.is_some() {
        return Err(X402PaymentError::SettlementFailed);
    }
    if response.extensions.is_some_and(|value| !value.is_empty()) {
        return Err(X402PaymentError::UnsupportedExtension);
    }
    let payer = response.payer.ok_or(X402PaymentError::CandidateMismatch)?;
    let payer = PublicEvmAddress::new(&payer).map_err(|_| X402PaymentError::CandidateMismatch)?;
    let transaction = validate_transaction_hash(&response.transaction)?;
    let expected_network = candidate.network().descriptor().caip2_id();
    if response.network != expected_network
        || payer.as_str() != candidate.decoded().account()
        || response
            .amount
            .as_deref()
            .is_some_and(|amount| amount != candidate.decoded().amount())
    {
        return Err(X402PaymentError::CandidateMismatch);
    }
    Ok(ValidatedX402PaymentResponse {
        transaction,
        network: response.network,
        payer: payer.as_str().to_owned(),
        amount: response.amount,
    })
}

/// Closed payment construction and response-validation failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum X402PaymentError {
    /// Signature scalars were not usable.
    InvalidSignature,
    /// Canonical JSON construction failed.
    EncodingFailed,
    /// An encoded or decoded header exceeded its fixed limit.
    HeaderLimit,
    /// Paid retry did not return a successful HTTP status.
    UnexpectedStatus,
    /// Successful response omitted settlement evidence.
    MissingResponse,
    /// More than one settlement header made the response ambiguous.
    DuplicateResponse,
    /// Settlement evidence was not canonical bounded v2 JSON.
    MalformedResponse,
    /// Server reported unsuccessful or ambiguous settlement.
    SettlementFailed,
    /// Response advertised an unsupported extension outcome.
    UnsupportedExtension,
    /// Network, payer or amount differed from the approved candidate.
    CandidateMismatch,
}

impl fmt::Display for X402PaymentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidSignature => "x402 signature is invalid",
            Self::EncodingFailed => "x402 payment payload encoding failed",
            Self::HeaderLimit => "x402 payment header exceeded its limit",
            Self::UnexpectedStatus => "x402 paid retry did not succeed",
            Self::MissingResponse => "x402 payment response is missing",
            Self::DuplicateResponse => "x402 payment response is ambiguous",
            Self::MalformedResponse => "x402 payment response is malformed",
            Self::SettlementFailed => "x402 settlement did not succeed",
            Self::UnsupportedExtension => "x402 settlement extension is unsupported",
            Self::CandidateMismatch => "x402 settlement differs from the approved payment",
        })
    }
}

impl Error for X402PaymentError {}

fn signature_hex(r: &[u8; 32], s: &[u8; 32], y_parity: bool) -> String {
    let mut bytes = [0_u8; 65];
    bytes[..32].copy_from_slice(r);
    bytes[32..64].copy_from_slice(s);
    bytes[64] = 27 + u8::from(y_parity);
    hex_prefixed(&bytes)
}

fn hex_prefixed(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(2 + bytes.len() * 2);
    value.push_str("0x");
    for byte in bytes {
        value.push(char::from(DIGITS[usize::from(byte >> 4)]));
        value.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    value
}

fn validate_transaction_hash(value: &str) -> Result<String, X402PaymentError> {
    if value.len() != 66
        || !value.starts_with("0x")
        || !value[2..].bytes().all(|byte| byte.is_ascii_hexdigit())
        || value[2..].bytes().all(|byte| byte == b'0')
    {
        return Err(X402PaymentError::MalformedResponse);
    }
    Ok(value.to_ascii_lowercase())
}

#[cfg(test)]
mod tests;
