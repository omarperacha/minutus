use std::error::Error;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use serde_json::{Value, json};

use super::{
    MAX_PAYMENT_REQUIRED_ENCODED_BYTES, PaymentRequiredParseError, parse_payment_required_response,
};

const RESOURCE_URL: &str = "https://api.example.com/premium-data?market=eth";
const ASSET: &str = "0x036cbd53842c5426634e7929541ec2318f3dcf7e";
const RECIPIENT: &str = "0x209693bc6afc0c5328ba36faf03c514ef312287c";

fn valid_challenge() -> Value {
    json!({
        "x402Version": 2,
        "error": "PAYMENT-SIGNATURE header is required",
        "resource": {
            "url": RESOURCE_URL,
            "description": "Access to premium market data",
            "mimeType": "application/json",
            "serviceName": "Example Market Data",
            "tags": ["market-data", "finance"],
            "iconUrl": "https://api.example.com/icon.png"
        },
        "accepts": [{
            "scheme": "exact",
            "network": "eip155:84532",
            "amount": "10000",
            "asset": ASSET,
            "payTo": RECIPIENT,
            "maxTimeoutSeconds": 60,
            "extra": {"name": "USDC", "version": "2"}
        }],
        "extensions": {}
    })
}

fn encoded(value: &Value) -> Result<Vec<u8>, Box<dyn Error>> {
    Ok(BASE64_STANDARD
        .encode(serde_json::to_vec(value)?)
        .into_bytes())
}

fn parse_value(
    value: &Value,
) -> Result<super::PaymentRequiredChallenge, PaymentRequiredParseError> {
    let encoded = encoded(value).map_err(|_| PaymentRequiredParseError::MalformedChallenge)?;
    parse_payment_required_response(402, &[encoded.as_slice()], RESOURCE_URL, 1_000)
}

#[test]
fn official_shape_is_parsed_and_bound_to_trusted_request() -> Result<(), Box<dyn Error>> {
    let challenge = parse_value(&valid_challenge())?;
    assert_eq!(challenge.version(), 2);
    assert_eq!(challenge.resource().canonical_url(), RESOURCE_URL);
    assert_eq!(challenge.resource().origin(), "https://api.example.com");
    assert_eq!(challenge.requirements().len(), 1);

    let requirement = &challenge.requirements()[0];
    assert_eq!(requirement.scheme(), "exact");
    assert_eq!(requirement.network(), "eip155:84532");
    assert_eq!(requirement.network_namespace(), "eip155");
    assert_eq!(requirement.network_reference(), "84532");
    assert_eq!(requirement.amount(), "10000");
    assert_eq!(requirement.asset(), ASSET);
    assert_eq!(requirement.recipient(), RECIPIENT);
    assert_eq!(requirement.max_timeout_seconds(), 60);
    assert_eq!(requirement.expires_at_unix_seconds(), 1_060);
    assert!(!requirement.is_expired_at(1_059));
    assert!(requirement.is_expired_at(1_060));
    assert_eq!(
        requirement
            .untrusted_extra()
            .and_then(|extra| extra.get("name")),
        Some(&json!("USDC"))
    );
    assert!(challenge.untrusted_extensions().is_some());
    Ok(())
}

#[test]
fn standard_base64_without_padding_is_accepted() -> Result<(), Box<dyn Error>> {
    let mut encoded = encoded(&valid_challenge())?;
    while encoded.last() == Some(&b'=') {
        encoded.pop();
    }
    let challenge =
        parse_payment_required_response(402, &[encoded.as_slice()], RESOURCE_URL, 1_000)?;
    assert_eq!(challenge.requirements().len(), 1);
    Ok(())
}

#[test]
fn response_envelope_must_be_unique_bounded_and_well_formed() -> Result<(), Box<dyn Error>> {
    let encoded = encoded(&valid_challenge())?;
    assert_eq!(
        parse_payment_required_response(200, &[encoded.as_slice()], RESOURCE_URL, 1_000).err(),
        Some(PaymentRequiredParseError::WrongStatus)
    );
    assert_eq!(
        parse_payment_required_response(402, &[], RESOURCE_URL, 1_000).err(),
        Some(PaymentRequiredParseError::MissingHeader)
    );
    assert_eq!(
        parse_payment_required_response(
            402,
            &[encoded.as_slice(), encoded.as_slice()],
            RESOURCE_URL,
            1_000,
        )
        .err(),
        Some(PaymentRequiredParseError::DuplicateHeader)
    );
    assert_eq!(
        parse_payment_required_response(402, &[b"%%%"], RESOURCE_URL, 1_000).err(),
        Some(PaymentRequiredParseError::InvalidBase64)
    );
    let oversized = vec![b'A'; MAX_PAYMENT_REQUIRED_ENCODED_BYTES + 1];
    assert_eq!(
        parse_payment_required_response(402, &[oversized.as_slice()], RESOURCE_URL, 1_000).err(),
        Some(PaymentRequiredParseError::EncodedHeaderLimit)
    );
    Ok(())
}

#[test]
fn duplicate_and_unknown_protocol_fields_are_rejected() {
    let duplicate_version = format!(
        r#"{{"x402Version":2,"x402Version":2,"resource":{{"url":"{RESOURCE_URL}"}},"accepts":[]}}"#
    );
    let encoded_duplicate = BASE64_STANDARD.encode(duplicate_version).into_bytes();
    assert_eq!(
        parse_payment_required_response(402, &[encoded_duplicate.as_slice()], RESOURCE_URL, 1_000,)
            .err(),
        Some(PaymentRequiredParseError::MalformedChallenge)
    );

    let mut unknown = valid_challenge();
    unknown["authority"] = json!(true);
    assert_eq!(
        parse_value(&unknown).err(),
        Some(PaymentRequiredParseError::MalformedChallenge)
    );

    let duplicate_extension = format!(
        r#"{{"x402Version":2,"resource":{{"url":"{RESOURCE_URL}"}},"accepts":[{{"scheme":"exact","network":"eip155:84532","amount":"10000","asset":"{ASSET}","payTo":"{RECIPIENT}","maxTimeoutSeconds":60,"extra":{{"name":"USDC","name":"other"}}}}]}}"#
    );
    let encoded_duplicate_extension = BASE64_STANDARD.encode(duplicate_extension).into_bytes();
    assert_eq!(
        parse_payment_required_response(
            402,
            &[encoded_duplicate_extension.as_slice()],
            RESOURCE_URL,
            1_000,
        )
        .err(),
        Some(PaymentRequiredParseError::MalformedChallenge)
    );
}

#[test]
fn version_and_requirement_cardinality_fail_closed() {
    let mut challenge = valid_challenge();
    challenge["x402Version"] = json!(1);
    assert_eq!(
        parse_value(&challenge).err(),
        Some(PaymentRequiredParseError::UnsupportedVersion)
    );

    challenge = valid_challenge();
    challenge["accepts"] = json!([]);
    assert_eq!(
        parse_value(&challenge).err(),
        Some(PaymentRequiredParseError::MissingRequirements)
    );

    challenge = valid_challenge();
    let requirement = challenge["accepts"][0].clone();
    challenge["accepts"] = Value::Array(vec![requirement; 9]);
    assert_eq!(
        parse_value(&challenge).err(),
        Some(PaymentRequiredParseError::RequirementLimit)
    );
}

#[test]
fn duplicate_payment_alternatives_are_ambiguous() {
    let mut challenge = valid_challenge();
    let requirement = challenge["accepts"][0].clone();
    challenge["accepts"] = json!([requirement.clone(), requirement]);
    assert_eq!(
        parse_value(&challenge).err(),
        Some(PaymentRequiredParseError::DuplicateRequirement)
    );
}

#[test]
fn advertised_resource_cannot_substitute_origin_or_url() -> Result<(), Box<dyn Error>> {
    let mut challenge = valid_challenge();
    challenge["resource"]["url"] = json!("https://evil.example/premium-data?market=eth");
    assert_eq!(
        parse_value(&challenge).err(),
        Some(PaymentRequiredParseError::ResourceOriginMismatch)
    );

    challenge = valid_challenge();
    challenge["resource"]["url"] = json!("https://api.example.com/other");
    assert_eq!(
        parse_value(&challenge).err(),
        Some(PaymentRequiredParseError::ResourceUrlMismatch)
    );

    let encoded = encoded(&valid_challenge())?;
    assert_eq!(
        parse_payment_required_response(
            402,
            &[encoded.as_slice()],
            "https://user@example.com/premium-data",
            1_000,
        )
        .err(),
        Some(PaymentRequiredParseError::InvalidOriginalResource)
    );
    Ok(())
}

#[test]
fn requirement_security_fields_are_validated_independently() {
    for (field, invalid, expected) in [
        (
            "scheme",
            json!("Exact Payment"),
            PaymentRequiredParseError::InvalidScheme,
        ),
        (
            "network",
            json!("eip155"),
            PaymentRequiredParseError::InvalidNetwork,
        ),
        (
            "network",
            json!("eip155:084532"),
            PaymentRequiredParseError::InvalidNetwork,
        ),
        (
            "asset",
            json!("USDC"),
            PaymentRequiredParseError::InvalidAsset,
        ),
        (
            "amount",
            json!("0"),
            PaymentRequiredParseError::InvalidAmount,
        ),
        (
            "amount",
            json!("01"),
            PaymentRequiredParseError::InvalidAmount,
        ),
        (
            "payTo",
            json!("merchant"),
            PaymentRequiredParseError::InvalidRecipient,
        ),
        (
            "maxTimeoutSeconds",
            json!(0),
            PaymentRequiredParseError::InvalidTimeout,
        ),
    ] {
        let mut challenge = valid_challenge();
        challenge["accepts"][0][field] = invalid;
        assert_eq!(parse_value(&challenge).err(), Some(expected), "{field}");
    }
}

#[test]
fn amount_accepts_uint256_max_and_rejects_overflow() {
    let mut challenge = valid_challenge();
    challenge["accepts"][0]["amount"] =
        json!("115792089237316195423570985008687907853269984665640564039457584007913129639935");
    assert!(parse_value(&challenge).is_ok());

    challenge["accepts"][0]["amount"] =
        json!("115792089237316195423570985008687907853269984665640564039457584007913129639936");
    assert_eq!(
        parse_value(&challenge).err(),
        Some(PaymentRequiredParseError::InvalidAmount)
    );
}

#[test]
fn timeout_derivation_rejects_expiry_overflow() -> Result<(), Box<dyn Error>> {
    let encoded = encoded(&valid_challenge())?;
    assert_eq!(
        parse_payment_required_response(402, &[encoded.as_slice()], RESOURCE_URL, u64::MAX - 59,)
            .err(),
        Some(PaymentRequiredParseError::ExpiryOverflow)
    );
    Ok(())
}

#[test]
fn non_evm_caip2_requirement_retains_bounded_generic_identifiers() -> Result<(), Box<dyn Error>> {
    let mut challenge = valid_challenge();
    challenge["accepts"][0]["network"] = json!("solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1");
    challenge["accepts"][0]["asset"] = json!("TokenMint111111111111111111111111111111111");
    challenge["accepts"][0]["payTo"] = json!("Merchant1111111111111111111111111111111111");
    let parsed = parse_value(&challenge)?;
    assert_eq!(parsed.requirements()[0].network_namespace(), "solana");
    Ok(())
}
