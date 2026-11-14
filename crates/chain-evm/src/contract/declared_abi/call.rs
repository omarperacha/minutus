use super::super::{ContractError, abi::hex_bytes};
use super::{layout, types, values};
use crate::PublicEvmAddress;
use alloy_dyn_abi::{DynSolCall, DynSolReturns, DynSolType};
use alloy_primitives::{Selector, keccak256};
use serde_json::Value;

/// Immutable bounded ABI wire facts for unsigned reads, never verified source,
/// complete effects, a signable transaction or account authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeclaredContractCall {
    target: PublicEvmAddress,
    signature: String,
    arguments: Vec<Value>,
    outputs: Vec<String>,
    abi: DynSolCall,
    calldata: String,
}
impl DeclaredContractCall {
    /// Resolves a full canonical signature and independently round-trips bytes.
    /// Integers/addresses/bytes are exact strings; tuples/arrays are JSON arrays.
    ///
    /// # Errors
    /// Rejects aliases, ambiguous signatures, unsupported types and resource
    /// limits. Caller-declared ABI is always unverified interpretation.
    pub fn new(
        target: &str,
        signature: &str,
        arguments: &[Value],
        outputs: &[String],
    ) -> Result<Self, ContractError> {
        let target = PublicEvmAddress::new(target).map_err(|_| ContractError::InvalidAddress)?;
        if target.as_str() == "0x0000000000000000000000000000000000000000" {
            return Err(ContractError::InvalidAddress);
        }
        let (name, tail) = signature
            .split_once('(')
            .ok_or(ContractError::UnsupportedAbi)?;
        if signature.len() > 512
            || name.is_empty()
            || !name
                .bytes()
                .enumerate()
                .all(|(i, b)| b == b'_' || b.is_ascii_alphabetic() || (i > 0 && b.is_ascii_digit()))
            || outputs.len() > 16
        {
            return Err(ContractError::UnsupportedAbi);
        }
        let DynSolType::Tuple(inputs) = types::parse(&format!("({tail}"))? else {
            return Err(ContractError::UnsupportedAbi);
        };
        if arguments.len() != inputs.len()
            || serde_json::to_vec(arguments)
                .map_err(|_| ContractError::InvalidArguments)?
                .len()
                > 16_384
        {
            return Err(ContractError::InvalidArguments);
        }
        let returns = outputs
            .iter()
            .map(|ty| types::parse(ty))
            .collect::<Result<Vec<_>, _>>()?;
        let abi = DynSolCall::new(
            Selector::from_slice(&keccak256(signature)[..4]),
            inputs,
            Some(name.into()),
            DynSolReturns::new(returns),
        );
        let mut nodes = 0;
        let args = abi
            .types()
            .iter()
            .zip(arguments)
            .map(|(ty, value)| values::input(ty, value, &mut nodes))
            .collect::<Result<Vec<_>, _>>()?;
        let encoded = abi
            .abi_encode_input(&args)
            .map_err(|_| ContractError::InvalidArguments)?;
        if encoded.len() > super::MAX_BYTES {
            return Err(ContractError::InvalidArguments);
        }
        let arguments = args.iter().map(values::output).collect();
        let result = Self {
            target,
            signature: signature.into(),
            arguments,
            outputs: outputs.to_vec(),
            abi,
            calldata: hex_bytes(&encoded),
        };
        result.verify_calldata(result.calldata())?;
        Ok(result)
    }
    /// Exact public application target.
    #[must_use]
    pub fn target(&self) -> &str {
        self.target.as_str()
    }
    /// Canonical overload-qualified signature.
    #[must_use]
    pub fn signature(&self) -> &str {
        &self.signature
    }
    /// Canonical typed arguments; strings remain untrusted content.
    #[must_use]
    pub fn arguments(&self) -> &[Value] {
        &self.arguments
    }
    /// Exact calldata, generated and round-tripped by maintained Alloy code.
    #[must_use]
    pub fn calldata(&self) -> &str {
        &self.calldata
    }
    /// Caller-declared canonical output types.
    #[must_use]
    pub fn outputs(&self) -> &[String] {
        &self.outputs
    }
    /// Independently decodes the complete exact payload and compares arguments.
    ///
    /// # Errors
    /// Rejects selector/value changes and every non-canonical/trailing layout.
    pub fn verify_calldata(&self, data: &str) -> Result<(), ContractError> {
        let bytes = decode_hex(data)?;
        if bytes.get(..4) != Some(self.abi.selector().as_slice()) {
            return Err(ContractError::MalformedData);
        }
        layout::guard(self.abi.types(), &bytes[4..])?;
        let values = self
            .abi
            .abi_decode_input(&bytes[4..])
            .map_err(|_| ContractError::MalformedData)?;
        if self
            .abi
            .abi_encode_input(&values)
            .map_err(|_| ContractError::MalformedData)?
            != bytes
            || values.iter().map(values::output).collect::<Vec<_>>() != self.arguments
        {
            return Err(ContractError::MalformedData);
        }
        Ok(())
    }
    /// Decodes bounded return bytes, preserving exact integers and typed values.
    ///
    /// # Errors
    /// Resource preflight precedes Alloy decoding; complete canonical re-encoding
    /// must match, including padding, offsets, lengths and full consumption.
    pub fn decode_result(&self, data: &str) -> Result<Vec<Value>, ContractError> {
        let bytes = decode_hex(data)?;
        layout::guard(self.abi.returns().types(), &bytes)?;
        let values = self
            .abi
            .abi_decode_output(&bytes)
            .map_err(|_| ContractError::MalformedData)?;
        if self
            .abi
            .abi_encode_output(&values)
            .map_err(|_| ContractError::MalformedData)?
            != bytes
        {
            return Err(ContractError::MalformedData);
        }
        Ok(values.iter().map(values::output).collect())
    }
}
fn decode_hex(data: &str) -> Result<Vec<u8>, ContractError> {
    let hex = data
        .strip_prefix("0x")
        .ok_or(ContractError::MalformedData)?;
    if hex.len() > super::MAX_BYTES * 2
        || hex.len() % 2 != 0
        || !hex.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(ContractError::MalformedData);
    }
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            std::str::from_utf8(pair)
                .ok()
                .and_then(|text| u8::from_str_radix(text, 16).ok())
                .ok_or(ContractError::MalformedData)
        })
        .collect()
}
