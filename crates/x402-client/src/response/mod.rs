//! Trusted parsing of bounded x402 v2 payment-required responses.

mod parse;
mod types;

pub(crate) use parse::UnambiguousJson;

pub use parse::{
    MAX_PAYMENT_REQUIRED_DECODED_BYTES, MAX_PAYMENT_REQUIRED_ENCODED_BYTES,
    MAX_PAYMENT_REQUIREMENTS, X402_V2, parse_payment_required_response,
};
pub use types::{
    PaymentRequiredChallenge, PaymentRequiredParseError, PaymentRequirement, ResourceBinding,
};

#[cfg(test)]
mod tests;
