use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};
use sha3::{Digest, Keccak256};

use crate::{PublicEvmAddress, rpc::Uint256};

const MAX_PARAMETERS: usize = 16;

/// Initially supported canonical ABI scalar types. Dynamic layouts, aliases,
/// tuples and arrays fail closed rather than being partially decoded.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ContractScalar {
    /// Exact unsigned decimal integer with 256-bit range.
    #[serde(rename = "uint256")]
    Uint256,
    /// Canonically padded 20-byte address.
    #[serde(rename = "address")]
    Address,
    /// Boolean encoded as exactly zero or one.
    #[serde(rename = "bool")]
    Bool,
    /// Exactly 32 opaque bytes.
    #[serde(rename = "bytes32")]
    Bytes32,
}

impl ContractScalar {
    fn parse(value: &str) -> Result<Self, ContractError> {
        match value {
            "uint256" => Ok(Self::Uint256),
            "address" => Ok(Self::Address),
            "bool" => Ok(Self::Bool),
            "bytes32" => Ok(Self::Bytes32),
            _ => Err(ContractError::UnsupportedAbi),
        }
    }

    fn encode(self, value: &str) -> Result<String, ContractError> {
        match self {
            Self::Uint256 => {
                if value.len() > 78 || (value.len() > 1 && value.starts_with('0')) {
                    return Err(ContractError::InvalidArguments);
                }
                Uint256::from_decimal(value)
                    .map(Uint256::to_abi_word)
                    .map_err(|_| ContractError::InvalidArguments)
            }
            Self::Address => PublicEvmAddress::new(value)
                .map(|address| format!("{:0>64}", &address.as_str()[2..]))
                .map_err(|_| ContractError::InvalidArguments),
            Self::Bool => match value {
                "false" => Ok(format!("{:0>64}", "0")),
                "true" => Ok(format!("{:0>64}", "1")),
                _ => Err(ContractError::InvalidArguments),
            },
            Self::Bytes32 if valid_hex(value, 32) => Ok(value[2..].to_ascii_lowercase()),
            Self::Bytes32 => Err(ContractError::InvalidArguments),
        }
    }

    fn decode(self, word: &str) -> Result<String, ContractError> {
        match self {
            Self::Uint256 => Uint256::from_abi_word(&format!("0x{word}"))
                .map(Uint256::to_decimal)
                .map_err(|_| ContractError::MalformedData),
            Self::Address if word[..24].bytes().all(|byte| byte == b'0') => {
                Ok(format!("0x{}", &word[24..]))
            }
            Self::Bool if word[..63].bytes().all(|byte| byte == b'0') => match &word[63..] {
                "0" => Ok("false".into()),
                "1" => Ok("true".into()),
                _ => Err(ContractError::MalformedData),
            },
            Self::Bytes32 => Ok(format!("0x{word}")),
            _ => Err(ContractError::MalformedData),
        }
    }
}

/// Immutable independently round-tripped wire facts; never an approved or
/// source-qualified call. No nonce, gas policy, digest or RPC URL is accepted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContractCall {
    target: PublicEvmAddress,
    signature: String,
    inputs: Vec<ContractScalar>,
    arguments: Vec<String>,
    outputs: Vec<ContractScalar>,
    calldata: String,
}

impl ContractCall {
    /// Constructs one exact, bounded function call from a canonical signature.
    ///
    /// # Errors
    /// Rejects non-canonical/unsupported ABI, invalid arguments and zero targets.
    pub fn new(
        target: &str,
        signature: &str,
        arguments: &[String],
        outputs: &[ContractScalar],
    ) -> Result<Self, ContractError> {
        let target = PublicEvmAddress::new(target).map_err(|_| ContractError::InvalidAddress)?;
        if target.as_str() == "0x0000000000000000000000000000000000000000" {
            return Err(ContractError::InvalidAddress);
        }
        let inputs = signature_types(signature)?;
        if arguments.len() != inputs.len() || outputs.len() > MAX_PARAMETERS {
            return Err(ContractError::InvalidArguments);
        }
        let digest = Keccak256::digest(signature.as_bytes());
        let selector = hex_bytes(&digest[..4]);
        let mut calldata = selector;
        for (kind, argument) in inputs.iter().zip(arguments) {
            calldata.push_str(&kind.encode(argument)?);
        }
        // Decode the complete payload independently: selector and exact length
        // must match, and each word must have canonical padding/range.
        let decoded = decode_words(&inputs, &format!("0x{}", &calldata[10..]))?;
        let result = Self {
            target,
            signature: signature.to_owned(),
            inputs,
            arguments: decoded,
            outputs: outputs.to_vec(),
            calldata,
        };
        result.verify_calldata(&result.calldata)?;
        Ok(result)
    }

    /// Exact target address, not a label or source-verification claim.
    #[must_use]
    pub fn target(&self) -> &str {
        self.target.as_str()
    }

    /// Full overload-qualified canonical signature.
    #[must_use]
    pub fn signature(&self) -> &str {
        &self.signature
    }

    /// Canonical public argument values, in function order.
    #[must_use]
    pub fn arguments(&self) -> &[String] {
        &self.arguments
    }

    /// Exact canonical selector and argument encoding.
    #[must_use]
    pub fn calldata(&self) -> &str {
        &self.calldata
    }

    /// Caller-declared result interpretation; never verified contract semantics.
    #[must_use]
    pub fn outputs(&self) -> &[ContractScalar] {
        &self.outputs
    }

    /// Independently checks exact selector, argument values and full consumption.
    ///
    /// # Errors
    /// Rejects changed selector/arguments, malformed padding and trailing bytes.
    pub fn verify_calldata(&self, calldata: &str) -> Result<(), ContractError> {
        if !valid_hex(calldata, 4 + self.inputs.len() * 32)
            || calldata[..10] != self.calldata[..10]
            || decode_words(&self.inputs, &format!("0x{}", &calldata[10..]))? != self.arguments
        {
            return Err(ContractError::MalformedData);
        }
        Ok(())
    }

    /// Strictly decodes the entire return payload using the declared types.
    ///
    /// # Errors
    /// Rejects truncation, trailing bytes and non-canonical scalar values.
    pub fn decode_result(&self, data: &str) -> Result<Vec<String>, ContractError> {
        decode_words(&self.outputs, data)
    }
}

fn signature_types(signature: &str) -> Result<Vec<ContractScalar>, ContractError> {
    if signature.len() > 512 {
        return Err(ContractError::UnsupportedAbi);
    }
    let (name, tail) = signature
        .split_once('(')
        .ok_or(ContractError::UnsupportedAbi)?;
    let mut bytes = name.bytes();
    if !bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        || !bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(ContractError::UnsupportedAbi);
    }
    let types = tail
        .strip_suffix(')')
        .ok_or(ContractError::UnsupportedAbi)?;
    if types.is_empty() {
        return Ok(Vec::new());
    }
    let types = types
        .split(',')
        .map(ContractScalar::parse)
        .collect::<Result<Vec<_>, _>>()?;
    if types.len() > MAX_PARAMETERS {
        return Err(ContractError::UnsupportedAbi);
    }
    Ok(types)
}

fn decode_words(types: &[ContractScalar], data: &str) -> Result<Vec<String>, ContractError> {
    if !valid_hex(data, types.len() * 32) {
        return Err(ContractError::MalformedData);
    }
    let data = data[2..].to_ascii_lowercase();
    types
        .iter()
        .enumerate()
        .map(|(index, kind)| kind.decode(&data[index * 64..(index + 1) * 64]))
        .collect()
}

pub(super) fn valid_hex(value: &str, bytes: usize) -> bool {
    value.strip_prefix("0x").is_some_and(|hex| {
        hex.len() == bytes * 2 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

pub(super) fn hex_bytes(bytes: &[u8]) -> String {
    use fmt::Write as _;
    let mut output = String::from("0x");
    for byte in bytes {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

/// Sanitised bounded failure, excluding provider prose and endpoint credentials.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContractError {
    /// Host cancelled before a subsequent RPC dispatch.
    Cancelled,
    /// Invalid target or caller address.
    InvalidAddress,
    /// ABI layout/signature is unsupported or ambiguous.
    UnsupportedAbi,
    /// Arguments are not canonical values for the declared scalar types.
    InvalidArguments,
    /// Provider returned malformed/non-canonical data.
    MalformedData,
    /// Provider transport, unsupported method or bounded response failure.
    ProviderUnavailable,
    /// The endpoint's chain does not match the selected network.
    WrongChain,
    /// Requested block missing or canonicality changed during observation.
    NonCanonicalBlock,
    /// Target has no runtime code at the observation block.
    NoCode,
}
impl fmt::Display for ContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Cancelled => "Contract observation cancelled before RPC dispatch",
            Self::InvalidAddress => "Invalid contract target or public caller address",
            Self::UnsupportedAbi => "Use supported canonical ABI types within the bounded argument, nesting and payload limits",
            Self::InvalidArguments => "Arguments must match the declared signature with exact canonical typed values",
            Self::MalformedData => "The contract/provider data is malformed or does not match the declared ABI",
            Self::ProviderUnavailable => "The configured RPC could not supply this bounded canonical observation; no fallback was used",
            Self::WrongChain => "The configured RPC returned the wrong chain",
            Self::NonCanonicalBlock => "The requested block is missing or no longer canonical; fetch a fresh observation",
            Self::NoCode => "The target has no contract runtime code at this block",
        })
    }
}
impl Error for ContractError {}

#[cfg(test)]
#[path = "abi_tests.rs"]
mod tests;
