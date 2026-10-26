use serde::Serialize;
use sha3::{Digest as _, Keccak256};

use super::types::{PreparedFields, X402PreparationError};

const DOMAIN_VERSION: &str = "2";
const DOMAIN_TYPE: &str =
    "EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)";
const AUTHORIZATION_TYPE: &str = "TransferWithAuthorization(address from,address to,uint256 value,uint256 validAfter,uint256 validBefore,bytes32 nonce)";

#[derive(Serialize)]
struct TypedData<'a> {
    types: Types,
    #[serde(rename = "primaryType")]
    primary_type: &'static str,
    domain: Domain<'a>,
    message: Message<'a>,
}

#[derive(Serialize)]
struct Types {
    #[serde(rename = "EIP712Domain")]
    domain: [TypeField; 4],
    #[serde(rename = "TransferWithAuthorization")]
    authorization: [TypeField; 6],
}

#[derive(Clone, Copy, Serialize)]
struct TypeField {
    name: &'static str,
    #[serde(rename = "type")]
    kind: &'static str,
}

#[derive(Serialize)]
struct Domain<'a> {
    name: &'static str,
    version: &'static str,
    #[serde(rename = "chainId")]
    chain_id: u64,
    #[serde(rename = "verifyingContract")]
    verifying_contract: &'a str,
}

#[derive(Serialize)]
struct Message<'a> {
    from: &'a str,
    to: &'a str,
    value: &'a str,
    #[serde(rename = "validAfter")]
    valid_after: String,
    #[serde(rename = "validBefore")]
    valid_before: String,
    nonce: String,
}

pub(super) fn canonical_typed_data(
    fields: &PreparedFields<'_>,
) -> Result<String, X402PreparationError> {
    serde_json::to_string(&TypedData {
        types: Types {
            domain: [
                TypeField {
                    name: "name",
                    kind: "string",
                },
                TypeField {
                    name: "version",
                    kind: "string",
                },
                TypeField {
                    name: "chainId",
                    kind: "uint256",
                },
                TypeField {
                    name: "verifyingContract",
                    kind: "address",
                },
            ],
            authorization: [
                TypeField {
                    name: "from",
                    kind: "address",
                },
                TypeField {
                    name: "to",
                    kind: "address",
                },
                TypeField {
                    name: "value",
                    kind: "uint256",
                },
                TypeField {
                    name: "validAfter",
                    kind: "uint256",
                },
                TypeField {
                    name: "validBefore",
                    kind: "uint256",
                },
                TypeField {
                    name: "nonce",
                    kind: "bytes32",
                },
            ],
        },
        primary_type: "TransferWithAuthorization",
        domain: Domain {
            name: crate::policy::usdc_domain_name(fields.chain_id),
            version: DOMAIN_VERSION,
            chain_id: fields.chain_id,
            verifying_contract: fields.asset,
        },
        message: Message {
            from: fields.account,
            to: fields.recipient,
            value: fields.amount,
            valid_after: fields.valid_after.to_string(),
            valid_before: fields.valid_before.to_string(),
            nonce: hex_prefixed(&fields.nonce),
        },
    })
    .map_err(|_| X402PreparationError::EncodingFailed)
}

pub(super) fn eip712_signing_digest(
    fields: &PreparedFields<'_>,
) -> Result<[u8; 32], X402PreparationError> {
    let domain_separator = hash_words(&[
        Keccak256::digest(DOMAIN_TYPE.as_bytes()).into(),
        Keccak256::digest(crate::policy::usdc_domain_name(fields.chain_id).as_bytes()).into(),
        Keccak256::digest(DOMAIN_VERSION.as_bytes()).into(),
        u64_word(fields.chain_id),
        address_word(fields.asset)?,
    ]);
    let authorization_hash = hash_words(&[
        Keccak256::digest(AUTHORIZATION_TYPE.as_bytes()).into(),
        address_word(fields.account)?,
        address_word(fields.recipient)?,
        decimal_word(fields.amount)?,
        u64_word(fields.valid_after),
        u64_word(fields.valid_before),
        fields.nonce,
    ]);
    let mut signing = Keccak256::new();
    signing.update([0x19, 0x01]);
    signing.update(domain_separator);
    signing.update(authorization_hash);
    Ok(signing.finalize().into())
}

fn hash_words(words: &[[u8; 32]]) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    for word in words {
        hasher.update(word);
    }
    hasher.finalize().into()
}

fn u64_word(value: u64) -> [u8; 32] {
    let mut word = [0_u8; 32];
    word[24..].copy_from_slice(&value.to_be_bytes());
    word
}

fn address_word(value: &str) -> Result<[u8; 32], X402PreparationError> {
    let hex = value
        .strip_prefix("0x")
        .filter(|hex| hex.len() == 40)
        .ok_or(X402PreparationError::EncodingFailed)?;
    let mut word = [0_u8; 32];
    decode_hex(hex, &mut word[12..])?;
    Ok(word)
}

fn decimal_word(value: &str) -> Result<[u8; 32], X402PreparationError> {
    let mut word = [0_u8; 32];
    for digit in value.bytes() {
        let digit = digit
            .checked_sub(b'0')
            .filter(|digit| *digit <= 9)
            .ok_or(X402PreparationError::EncodingFailed)?;
        let mut carry = u16::from(digit);
        for byte in word.iter_mut().rev() {
            let next = u16::from(*byte) * 10 + carry;
            *byte = next.to_le_bytes()[0];
            carry = next >> 8;
        }
        if carry != 0 {
            return Err(X402PreparationError::EncodingFailed);
        }
    }
    Ok(word)
}

fn decode_hex(value: &str, output: &mut [u8]) -> Result<(), X402PreparationError> {
    if value.len() != output.len() * 2 {
        return Err(X402PreparationError::EncodingFailed);
    }
    for (pair, byte) in value.as_bytes().chunks_exact(2).zip(output) {
        *byte = (hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?;
    }
    Ok(())
}

fn hex_nibble(value: u8) -> Result<u8, X402PreparationError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(X402PreparationError::EncodingFailed),
    }
}

fn hex_prefixed(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut encoded = String::with_capacity(2 + bytes.len() * 2);
    encoded.push_str("0x");
    for byte in bytes {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}
