use chain_evm::PublicEvmAddress;
use serde_json::{Map, Value};

use super::types::{PreparedFields, X402AuthorizationFacts, X402PreparationError};

const ROOT_KEYS: &[&str] = &["types", "primaryType", "domain", "message"];
const DOMAIN_KEYS: &[&str] = &["name", "version", "chainId", "verifyingContract"];
const MESSAGE_KEYS: &[&str] = &["from", "to", "value", "validAfter", "validBefore", "nonce"];

pub(super) fn decode_and_verify_authorization(
    typed_data: &str,
    expected: &PreparedFields<'_>,
) -> Result<X402AuthorizationFacts, X402PreparationError> {
    let root = serde_json::from_str::<Value>(typed_data)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .ok_or(X402PreparationError::CandidateMismatch)?;
    if !has_exact_keys(&root, ROOT_KEYS)
        || root.get("primaryType").and_then(Value::as_str) != Some("TransferWithAuthorization")
        || !exact_types(root.get("types"))
    {
        return Err(X402PreparationError::CandidateMismatch);
    }
    let domain = root
        .get("domain")
        .and_then(Value::as_object)
        .filter(|domain| has_exact_keys(domain, DOMAIN_KEYS))
        .ok_or(X402PreparationError::CandidateMismatch)?;
    let message = root
        .get("message")
        .and_then(Value::as_object)
        .filter(|message| has_exact_keys(message, MESSAGE_KEYS))
        .ok_or(X402PreparationError::CandidateMismatch)?;

    let account = normalized_address(message.get("from"))?;
    let recipient = normalized_address(message.get("to"))?;
    let asset = normalized_address(domain.get("verifyingContract"))?;
    let amount = canonical_decimal(message.get("value"))?;
    let valid_after = decimal_u64(message.get("validAfter"))?;
    let valid_before = decimal_u64(message.get("validBefore"))?;
    let nonce = nonce(message.get("nonce"))?;
    let chain_id = domain
        .get("chainId")
        .and_then(Value::as_u64)
        .ok_or(X402PreparationError::CandidateMismatch)?;

    if domain.get("name").and_then(Value::as_str)
        != Some(crate::policy::usdc_domain_name(expected.chain_id))
        || domain.get("version").and_then(Value::as_str) != Some("2")
        || chain_id != expected.chain_id
        || asset != expected.asset
        || account != expected.account
        || recipient != expected.recipient
        || amount != expected.amount
        || valid_after != expected.valid_after
        || valid_before != expected.valid_before
        || nonce != expected.nonce
    {
        return Err(X402PreparationError::CandidateMismatch);
    }
    Ok(X402AuthorizationFacts::new(expected))
}

fn exact_types(value: Option<&Value>) -> bool {
    let Some(types) = value.and_then(Value::as_object) else {
        return false;
    };
    has_exact_keys(types, &["EIP712Domain", "TransferWithAuthorization"])
        && exact_type(
            types.get("EIP712Domain"),
            &[
                ("name", "string"),
                ("version", "string"),
                ("chainId", "uint256"),
                ("verifyingContract", "address"),
            ],
        )
        && exact_type(
            types.get("TransferWithAuthorization"),
            &[
                ("from", "address"),
                ("to", "address"),
                ("value", "uint256"),
                ("validAfter", "uint256"),
                ("validBefore", "uint256"),
                ("nonce", "bytes32"),
            ],
        )
}

fn exact_type(value: Option<&Value>, expected: &[(&str, &str)]) -> bool {
    value.and_then(Value::as_array).is_some_and(|fields| {
        fields.len() == expected.len()
            && fields.iter().zip(expected).all(|(field, (name, kind))| {
                field.as_object().is_some_and(|field| {
                    has_exact_keys(field, &["name", "type"])
                        && field.get("name").and_then(Value::as_str) == Some(*name)
                        && field.get("type").and_then(Value::as_str) == Some(*kind)
                })
            })
    })
}

fn has_exact_keys(object: &Map<String, Value>, expected: &[&str]) -> bool {
    object.len() == expected.len() && expected.iter().all(|key| object.contains_key(*key))
}

fn normalized_address(value: Option<&Value>) -> Result<String, X402PreparationError> {
    value
        .and_then(Value::as_str)
        .and_then(|address| PublicEvmAddress::new(address).ok())
        .map(|address| address.as_str().to_owned())
        .ok_or(X402PreparationError::CandidateMismatch)
}

fn canonical_decimal(value: Option<&Value>) -> Result<String, X402PreparationError> {
    value
        .and_then(Value::as_str)
        .filter(|value| {
            !value.is_empty()
                && value.bytes().all(|byte| byte.is_ascii_digit())
                && (*value == "0" || !value.starts_with('0'))
        })
        .map(str::to_owned)
        .ok_or(X402PreparationError::CandidateMismatch)
}

fn decimal_u64(value: Option<&Value>) -> Result<u64, X402PreparationError> {
    canonical_decimal(value)?
        .parse()
        .map_err(|_| X402PreparationError::CandidateMismatch)
}

fn nonce(value: Option<&Value>) -> Result<[u8; 32], X402PreparationError> {
    let value = value
        .and_then(Value::as_str)
        .and_then(|value| value.strip_prefix("0x"))
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or(X402PreparationError::CandidateMismatch)?;
    let mut nonce = [0_u8; 32];
    for (pair, byte) in value.as_bytes().chunks_exact(2).zip(&mut nonce) {
        *byte = (nibble(pair[0])? << 4) | nibble(pair[1])?;
    }
    Ok(nonce)
}

fn nibble(value: u8) -> Result<u8, X402PreparationError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        _ => Err(X402PreparationError::CandidateMismatch),
    }
}
