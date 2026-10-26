use x402_client::{
    MIN_X402_PREPARATION_LIFETIME_SECONDS, PaymentRequiredParseError, X402PolicyError,
    admit_payment_requirement, parse_payment_required_response, validate_x402_payment_terms,
};

use super::transport::{Observation, ResourceUrl};

#[derive(Clone, Copy)]
pub(super) enum Expectation {
    Valid,
    Malformed,
    ShortLived,
    Redirect,
}

impl Expectation {
    pub(super) fn parse(value: &str) -> Result<Self, &'static str> {
        match value {
            "valid" => Ok(Self::Valid),
            "malformed" => Ok(Self::Malformed),
            "short-lived" => Ok(Self::ShortLived),
            "redirect" => Ok(Self::Redirect),
            _ => Err("unknown preflight expectation"),
        }
    }
}

pub(super) fn inspect(
    resource: &ResourceUrl,
    observation: &Observation,
    expected: Expectation,
    received_at: u64,
) -> Result<&'static str, &'static str> {
    if matches!(expected, Expectation::Redirect) {
        return if observation.status == 307 {
            Ok("PASS: diagnostic redirect observed without following it.")
        } else {
            Err("expected an unfollowed 307 redirect")
        };
    }
    if observation.status != 402 {
        return Err("expected HTTP 402; check the run URL, emulator configuration and run expiry");
    }
    let headers: Vec<&[u8]> = observation
        .payment_required
        .iter()
        .map(Vec::as_slice)
        .collect();
    let parsed = parse_payment_required_response(402, &headers, resource.as_str(), received_at);
    if matches!(expected, Expectation::Malformed) {
        return if matches!(parsed, Err(PaymentRequiredParseError::InvalidBase64)) {
            Ok("PASS: malformed Base64 challenge rejected before preparation.")
        } else {
            Err("expected the backend malformed_challenge fixture (invalid Base64)")
        };
    }
    let challenge =
        parsed.map_err(|_| "x402 v2 challenge parsing or exact resource binding failed")?;
    if challenge.requirements().len() != 1 {
        return Err("diagnostic fixture must offer exactly one payment requirement");
    }
    validate_x402_payment_terms(&challenge, 0, received_at)
        .map_err(|_| "unsupported x402 scheme, Base Sepolia USDC terms, extensions or expiry")?;
    if !matches!(
        admit_payment_requirement(&challenge, 0, received_at),
        Err(X402PolicyError::InsecureOrigin)
    ) {
        return Err("production policy unexpectedly admitted a loopback payment");
    }
    let requirement = &challenge.requirements()[0];
    if requirement.amount() != "10000" {
        return Err("initial diagnostic price must be exactly 0.01 test USDC");
    }
    if matches!(expected, Expectation::ShortLived) {
        if requirement.max_timeout_seconds() != 1
            || requirement.max_timeout_seconds() >= MIN_X402_PREPARATION_LIFETIME_SECONDS
            || validate_x402_payment_terms(&challenge, 0, requirement.expires_at_unix_seconds())
                != Err(X402PolicyError::ExpiredRequirement)
        {
            return Err("expected a one-second quote rejected at the local expiry boundary");
        }
        return Ok(
            "PASS: one-second quote is too short for preparation and expires locally; server expiry is not exercised.",
        );
    }
    if requirement.max_timeout_seconds() != 60 {
        return Err("initial diagnostic authorization window must be 60 seconds");
    }
    Ok(
        "PASS: exact-resource x402 v2 / exact / Base Sepolia / USDC / 0.01 / 60-second terms; production origin policy remains closed.",
    )
}
