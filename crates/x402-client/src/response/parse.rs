use std::{collections::HashSet, fmt};

use base64::{
    Engine as _,
    engine::general_purpose::{
        STANDARD as BASE64_STANDARD, STANDARD_NO_PAD as BASE64_STANDARD_NO_PAD,
    },
};
use chain_evm::PublicEvmAddress;
use serde::{
    Deserialize, Deserializer,
    de::{MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Value};
use url::Url;

use super::{
    PaymentRequiredChallenge, PaymentRequiredParseError, PaymentRequirement, ResourceBinding,
};

/// x402 protocol version implemented by this parser.
pub const X402_V2: u8 = 2;
/// Maximum encoded bytes accepted from one `PAYMENT-REQUIRED` header.
pub const MAX_PAYMENT_REQUIRED_ENCODED_BYTES: usize = 8 * 1_024;
/// Maximum decoded JSON bytes accepted from one `PAYMENT-REQUIRED` header.
pub const MAX_PAYMENT_REQUIRED_DECODED_BYTES: usize = 8 * 1_024;
/// Maximum payment alternatives accepted from one challenge.
pub const MAX_PAYMENT_REQUIREMENTS: usize = 8;

const MAX_SCHEME_BYTES: usize = 32;
const MAX_GENERIC_IDENTIFIER_BYTES: usize = 128;
const MAX_UINT256_DECIMAL: &str =
    "115792089237316195423570985008687907853269984665640564039457584007913129639935";

/// Parses and validates one HTTP x402 v2 payment-required response.
///
/// `payment_required_headers` contains every raw value observed for the
/// case-insensitive `PAYMENT-REQUIRED` field. Supplying the full collection is
/// required so duplicate fields can fail closed. `received_at_unix_seconds` is
/// trusted host time and is used to derive each local expiry deadline.
///
/// This function validates syntax and exact request binding only. It does not
/// select a requirement, apply the domain/network allow-list, prepare a payment,
/// sign, or retry the HTTP request.
///
/// # Errors
///
/// Returns [`PaymentRequiredParseError`] when the response is not one bounded,
/// unambiguous x402 v2 challenge bound to `original_resource_url`.
pub fn parse_payment_required_response(
    status: u16,
    payment_required_headers: &[&[u8]],
    original_resource_url: &str,
    received_at_unix_seconds: u64,
) -> Result<PaymentRequiredChallenge, PaymentRequiredParseError> {
    if status != 402 {
        return Err(PaymentRequiredParseError::WrongStatus);
    }
    let encoded = match payment_required_headers {
        [] => return Err(PaymentRequiredParseError::MissingHeader),
        [encoded] => *encoded,
        [_, _, ..] => return Err(PaymentRequiredParseError::DuplicateHeader),
    };
    if encoded.is_empty() || encoded.len() > MAX_PAYMENT_REQUIRED_ENCODED_BYTES {
        return Err(PaymentRequiredParseError::EncodedHeaderLimit);
    }
    let decoded = decode_header(encoded)?;
    if decoded.is_empty() || decoded.len() > MAX_PAYMENT_REQUIRED_DECODED_BYTES {
        return Err(PaymentRequiredParseError::DecodedChallengeLimit);
    }

    serde_json::from_slice::<UnambiguousJson>(&decoded)
        .map_err(|_| PaymentRequiredParseError::MalformedChallenge)?;
    let raw: RawPaymentRequired = serde_json::from_slice(&decoded)
        .map_err(|_| PaymentRequiredParseError::MalformedChallenge)?;
    validate_raw_challenge(raw, original_resource_url, received_at_unix_seconds)
}

fn decode_header(encoded: &[u8]) -> Result<Vec<u8>, PaymentRequiredParseError> {
    BASE64_STANDARD
        .decode(encoded)
        .or_else(|_| BASE64_STANDARD_NO_PAD.decode(encoded))
        .map_err(|_| PaymentRequiredParseError::InvalidBase64)
}

fn validate_raw_challenge(
    raw: RawPaymentRequired,
    original_resource_url: &str,
    received_at_unix_seconds: u64,
) -> Result<PaymentRequiredChallenge, PaymentRequiredParseError> {
    if raw.version != u64::from(X402_V2) {
        return Err(PaymentRequiredParseError::UnsupportedVersion);
    }
    if raw.accepts.is_empty() {
        return Err(PaymentRequiredParseError::MissingRequirements);
    }
    if raw.accepts.len() > MAX_PAYMENT_REQUIREMENTS {
        return Err(PaymentRequiredParseError::RequirementLimit);
    }

    validate_remote_metadata(&raw)?;
    let resource = validate_resource_binding(original_resource_url, &raw.resource.url)?;
    let mut requirements = Vec::with_capacity(raw.accepts.len());
    for requirement in raw.accepts {
        let requirement = validate_requirement(requirement, received_at_unix_seconds)?;
        if requirements.contains(&requirement) {
            return Err(PaymentRequiredParseError::DuplicateRequirement);
        }
        requirements.push(requirement);
    }
    Ok(PaymentRequiredChallenge {
        resource,
        requirements,
        untrusted_extensions: raw.extensions,
    })
}

fn validate_remote_metadata(raw: &RawPaymentRequired) -> Result<(), PaymentRequiredParseError> {
    if raw.error.as_deref().is_some_and(has_disallowed_text) {
        return Err(PaymentRequiredParseError::MalformedChallenge);
    }
    if raw
        .resource
        .service_name
        .as_deref()
        .is_some_and(|value| !is_printable_ascii(value, 32))
    {
        return Err(PaymentRequiredParseError::MalformedChallenge);
    }
    if raw.resource.tags.as_ref().is_some_and(|tags| {
        tags.len() > 5 || tags.iter().any(|value| !is_printable_ascii(value, 32))
    }) {
        return Err(PaymentRequiredParseError::MalformedChallenge);
    }
    Ok(())
}

fn validate_resource_binding(
    original_resource_url: &str,
    advertised_resource_url: &str,
) -> Result<ResourceBinding, PaymentRequiredParseError> {
    let original = parse_http_resource_url(
        original_resource_url,
        PaymentRequiredParseError::InvalidOriginalResource,
    )?;
    let advertised = parse_http_resource_url(
        advertised_resource_url,
        PaymentRequiredParseError::InvalidAdvertisedResource,
    )?;
    let original_origin = original.origin().ascii_serialization();
    if advertised.origin().ascii_serialization() != original_origin {
        return Err(PaymentRequiredParseError::ResourceOriginMismatch);
    }
    if advertised.as_str() != original.as_str() {
        return Err(PaymentRequiredParseError::ResourceUrlMismatch);
    }
    Ok(ResourceBinding {
        canonical_url: original.into(),
        origin: original_origin,
    })
}

fn parse_http_resource_url(
    raw: &str,
    error: PaymentRequiredParseError,
) -> Result<Url, PaymentRequiredParseError> {
    let url = Url::parse(raw).map_err(|_| error)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(error);
    }
    Ok(url)
}

fn validate_requirement(
    raw: RawPaymentRequirement,
    received_at_unix_seconds: u64,
) -> Result<PaymentRequirement, PaymentRequiredParseError> {
    if !is_scheme(&raw.scheme) {
        return Err(PaymentRequiredParseError::InvalidScheme);
    }
    let network_namespace_len = validate_caip2(&raw.network)?;
    let amount = validate_amount(raw.amount)?;
    let (asset, recipient) = if &raw.network[..network_namespace_len] == "eip155" {
        validate_evm_requirement_addresses(&raw.asset, &raw.pay_to)?
    } else {
        (
            validate_generic_identifier(raw.asset, PaymentRequiredParseError::InvalidAsset)?,
            validate_generic_identifier(raw.pay_to, PaymentRequiredParseError::InvalidRecipient)?,
        )
    };
    if raw.max_timeout_seconds == 0 {
        return Err(PaymentRequiredParseError::InvalidTimeout);
    }
    let expires_at_unix_seconds = received_at_unix_seconds
        .checked_add(raw.max_timeout_seconds)
        .ok_or(PaymentRequiredParseError::ExpiryOverflow)?;
    Ok(PaymentRequirement {
        scheme: raw.scheme,
        network: raw.network,
        network_namespace_len,
        amount,
        asset,
        recipient,
        max_timeout_seconds: raw.max_timeout_seconds,
        expires_at_unix_seconds,
        untrusted_extra: raw.extra,
    })
}

fn validate_evm_requirement_addresses(
    asset: &str,
    recipient: &str,
) -> Result<(String, String), PaymentRequiredParseError> {
    let asset =
        PublicEvmAddress::new(asset).map_err(|_| PaymentRequiredParseError::InvalidAsset)?;
    let recipient = PublicEvmAddress::new(recipient)
        .map_err(|_| PaymentRequiredParseError::InvalidRecipient)?;
    Ok((asset.as_str().to_owned(), recipient.as_str().to_owned()))
}

fn validate_generic_identifier(
    value: String,
    error: PaymentRequiredParseError,
) -> Result<String, PaymentRequiredParseError> {
    if value.is_empty()
        || value.len() > MAX_GENERIC_IDENTIFIER_BYTES
        || !value.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err(error);
    }
    Ok(value)
}

fn validate_amount(amount: String) -> Result<String, PaymentRequiredParseError> {
    if amount.is_empty()
        || !amount.bytes().all(|byte| byte.is_ascii_digit())
        || amount == "0"
        || (amount.len() > 1 && amount.starts_with('0'))
        || amount.len() > MAX_UINT256_DECIMAL.len()
        || (amount.len() == MAX_UINT256_DECIMAL.len() && amount.as_str() > MAX_UINT256_DECIMAL)
    {
        return Err(PaymentRequiredParseError::InvalidAmount);
    }
    Ok(amount)
}

fn validate_caip2(network: &str) -> Result<usize, PaymentRequiredParseError> {
    let Some((namespace, reference)) = network.split_once(':') else {
        return Err(PaymentRequiredParseError::InvalidNetwork);
    };
    if !(3..=8).contains(&namespace.len())
        || !namespace
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        || reference.is_empty()
        || reference.len() > 32
        || !reference
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(PaymentRequiredParseError::InvalidNetwork);
    }
    if namespace == "eip155"
        && (!reference.bytes().all(|byte| byte.is_ascii_digit())
            || (reference.len() > 1 && reference.starts_with('0'))
            || reference.parse::<u64>().is_err())
    {
        return Err(PaymentRequiredParseError::InvalidNetwork);
    }
    Ok(namespace.len())
}

fn is_scheme(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_SCHEME_BYTES
        && !value.starts_with('-')
        && !value.ends_with('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn has_disallowed_text(value: &str) -> bool {
    value.len() > 512 || value.chars().any(char::is_control)
}

fn is_printable_ascii(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() || byte == b' ')
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPaymentRequired {
    #[serde(rename = "x402Version")]
    version: u64,
    error: Option<String>,
    resource: RawResource,
    accepts: Vec<RawPaymentRequirement>,
    extensions: Option<Map<String, Value>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawResource {
    url: String,
    #[serde(rename = "description")]
    _description: Option<String>,
    #[serde(rename = "mimeType")]
    _mime_type: Option<String>,
    #[serde(rename = "serviceName")]
    service_name: Option<String>,
    tags: Option<Vec<String>>,
    #[serde(rename = "iconUrl")]
    _icon_url: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPaymentRequirement {
    scheme: String,
    network: String,
    amount: String,
    asset: String,
    #[serde(rename = "payTo")]
    pay_to: String,
    #[serde(rename = "maxTimeoutSeconds")]
    max_timeout_seconds: u64,
    extra: Option<Map<String, Value>>,
}

pub(crate) struct UnambiguousJson;

impl<'de> Deserialize<'de> for UnambiguousJson {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(UnambiguousJsonVisitor)
    }
}

struct UnambiguousJsonVisitor;

impl<'de> Visitor<'de> for UnambiguousJsonVisitor {
    type Value = UnambiguousJson;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("JSON without duplicate object keys")
    }

    fn visit_bool<E>(self, _value: bool) -> Result<Self::Value, E> {
        Ok(UnambiguousJson)
    }

    fn visit_i64<E>(self, _value: i64) -> Result<Self::Value, E> {
        Ok(UnambiguousJson)
    }

    fn visit_u64<E>(self, _value: u64) -> Result<Self::Value, E> {
        Ok(UnambiguousJson)
    }

    fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E> {
        Ok(UnambiguousJson)
    }

    fn visit_str<E>(self, _value: &str) -> Result<Self::Value, E> {
        Ok(UnambiguousJson)
    }

    fn visit_string<E>(self, _value: String) -> Result<Self::Value, E> {
        Ok(UnambiguousJson)
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(UnambiguousJson)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(UnambiguousJson)
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        UnambiguousJson::deserialize(deserializer)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        while sequence.next_element::<UnambiguousJson>()?.is_some() {}
        Ok(UnambiguousJson)
    }

    fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut keys = HashSet::new();
        while let Some(key) = object.next_key::<String>()? {
            if !keys.insert(key) {
                return Err(serde::de::Error::custom("duplicate JSON object key"));
            }
            object.next_value::<UnambiguousJson>()?;
        }
        Ok(UnambiguousJson)
    }
}
