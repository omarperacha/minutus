use serde::Serialize;
use serde_json::{Value, json};
use sha3::{Digest, Keccak256};

use super::{
    ContractCall, ContractError, DeclaredContractCall,
    abi::{hex_bytes, valid_hex},
};
use crate::{
    EvmNetwork, EvmRpcUrl, PublicEvmAddress,
    rpc::{RpcCallOutcome, RpcClient, Uint256},
};

/// Explicit observation selector. `latest` is resolved afresh for every call.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContractBlock {
    /// Resolve the current latest block; no cache or fallback.
    Latest,
    /// Exact block hash, with canonicality required.
    Hash(String),
}
impl ContractBlock {
    /// Parses `latest` or a 32-byte block hash; never accepts a block height.
    ///
    /// # Errors
    /// Rejects malformed hashes, pending tags and unsupported selectors.
    pub fn new(value: &str) -> Result<Self, ContractError> {
        if value == "latest" {
            Ok(Self::Latest)
        } else if valid_hex(value, 32) {
            Ok(Self::Hash(value.to_ascii_lowercase()))
        } else {
            Err(ContractError::MalformedData)
        }
    }
}

/// Public code observation. Runtime identity is not ABI, proxy or effect trust.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractCodeObservation {
    /// EIP-155 identity checked against the configured provider.
    pub chain_id: u64,
    /// Exact contract address.
    pub target: String,
    /// Canonical block height, as an exact decimal string.
    pub block_number: String,
    /// Exact hash used in every state request.
    pub block_hash: String,
    /// Keccak-256 of the observed runtime.
    pub runtime_code_hash: String,
    /// Observed runtime length.
    pub runtime_code_bytes: usize,
    /// Always false: no verified source/ABI/proxy resolver is installed.
    pub source_qualified: bool,
    /// Always false: arbitrary effects are not qualified for signing.
    pub writes_available: bool,
    /// Provider-reported canonicality checked again after the observation.
    pub canonical_at_observation: bool,
}

/// Unsigned call execution; simulated state changes are discarded by `eth_call`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ContractReadOutcome<T = String> {
    /// Full payload strictly decoded against caller-declared, unverified types.
    Returned {
        /// Canonical typed values in output order; exact integers remain strings.
        values: Vec<T>,
        /// Complete raw ABI bytes; never instructions or verified semantics.
        data: String,
    },
    /// Provider reports a revert. Revert data is untrusted bytes, never prose.
    Reverted {
        /// Bounded integer provider error code.
        code: i64,
        /// Optional bounded hexadecimal revert payload.
        data: Option<String>,
        /// Optional canonical standard Error/Panic interpretation. Still untrusted.
        interpretation: Option<super::ContractRevert>,
    },
}

/// Immutable observation context. It grants neither approval nor task success.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractReadObservation<T = String, O = super::ContractScalar> {
    /// Code/chain/canonical block context.
    pub context: ContractCodeObservation,
    /// Simulated public caller; does not imply control of this account.
    pub caller: String,
    /// Canonical overload-qualified function signature.
    pub function: String,
    /// Canonical argument values, including bounded typed arrays/tuples.
    pub arguments: Vec<T>,
    /// Exact call bytes.
    pub calldata: String,
    /// Declared return types, not source-verified ABI.
    pub output_types: Vec<O>,
    /// Fixed zero native value; reads cannot transfer funds.
    pub value_wei: String,
    /// Host cap, not an estimate or transaction gas policy.
    pub gas_limit: u64,
    /// Typed execution or revert fact.
    pub outcome: ContractReadOutcome<T>,
}

trait ReadRpc {
    fn value(&self, method: &str, params: Value) -> Result<Value, ContractError>;
    fn call(&self, params: Value) -> Result<RpcCallOutcome, ContractError>;
}
impl ReadRpc for RpcClient<'_> {
    fn value(&self, method: &str, params: Value) -> Result<Value, ContractError> {
        self.value_result(method, &params)
            .map_err(|_| ContractError::ProviderUnavailable)
    }
    fn call(&self, params: Value) -> Result<RpcCallOutcome, ContractError> {
        self.call_outcome(&params)
            .map_err(|_| ContractError::ProviderUnavailable)
    }
}

struct AdmittedRpc<'a, R, F> {
    rpc: &'a R,
    admit: &'a F,
}
impl<R: ReadRpc, F: Fn() -> bool> ReadRpc for AdmittedRpc<'_, R, F> {
    fn value(&self, method: &str, params: Value) -> Result<Value, ContractError> {
        if !(self.admit)() {
            return Err(ContractError::Cancelled);
        }
        self.rpc.value(method, params)
    }
    fn call(&self, params: Value) -> Result<RpcCallOutcome, ContractError> {
        if !(self.admit)() {
            return Err(ContractError::Cancelled);
        }
        self.rpc.call(params)
    }
}

/// Fetches runtime identity without accessing a wallet or resolving ABI trust.
///
/// # Errors
/// Fails on wrong chain, absent/non-canonical block, missing/malformed code or RPC
/// failure or cancellation. Admission is checked before every RPC; an in-flight
/// request retains its bounded transport timeout. No provider/state fallback.
pub fn observe_contract_code_with_admission(
    network: EvmNetwork,
    endpoint: &EvmRpcUrl,
    target: &str,
    block: &ContractBlock,
    admit: &impl Fn() -> bool,
) -> Result<ContractCodeObservation, ContractError> {
    let target = PublicEvmAddress::new(target).map_err(|_| ContractError::InvalidAddress)?;
    let rpc = RpcClient::new(endpoint).map_err(|_| ContractError::ProviderUnavailable)?;
    let rpc = AdmittedRpc { rpc: &rpc, admit };
    let observation = code_at(&rpc, network, target.as_str(), block)?;
    check_canonical(&rpc, &observation)?;
    Ok(observation)
}

/// Performs one bounded unsigned zero-value call at an exact canonical block.
/// The default caller is the zero address, never the selected wallet account.
///
/// # Errors
/// Fails closed on provider, chain, canonicality and strict ABI decoding errors.
/// Reverts are a typed observation, not a claimed successful read. Admission
/// is checked before each RPC; in-flight requests retain their transport timeout.
pub fn read_contract_with_admission(
    network: EvmNetwork,
    endpoint: &EvmRpcUrl,
    call: &ContractCall,
    caller: Option<&str>,
    block: &ContractBlock,
    admit: &impl Fn() -> bool,
) -> Result<ContractReadObservation, ContractError> {
    let rpc = RpcClient::new(endpoint).map_err(|_| ContractError::ProviderUnavailable)?;
    read_at(
        &AdmittedRpc { rpc: &rpc, admit },
        network,
        call,
        caller,
        block,
    )
}

fn read_at(
    rpc: &impl ReadRpc,
    network: EvmNetwork,
    call: &ContractCall,
    caller: Option<&str>,
    block: &ContractBlock,
) -> Result<ContractReadObservation, ContractError> {
    call.verify_calldata(call.calldata())?;
    let (context, caller, result) =
        read_raw(rpc, network, call.target(), call.calldata(), caller, block)?;
    let outcome = match result {
        RpcCallOutcome::Succeeded(data) => ContractReadOutcome::Returned {
            values: call.decode_result(&data)?,
            data,
        },
        RpcCallOutcome::Reverted { code, data } => ContractReadOutcome::Reverted {
            code,
            interpretation: super::revert::decode(data.as_deref()),
            data,
        },
    };
    Ok(ContractReadObservation {
        context,
        caller,
        function: call.signature().into(),
        arguments: call.arguments().to_vec(),
        calldata: call.calldata().into(),
        output_types: call.outputs().to_vec(),
        value_wei: "0".into(),
        gas_limit: 1_000_000,
        outcome,
    })
}

/// Performs a bounded read with canonical caller-declared dynamic ABI types.
/// Source, proxy, function names and effect meanings remain unverified.
///
/// # Errors
/// Rejects malformed or excessive ABI layouts before decoding, as well as the
/// same provider, admission and canonical block faults as scalar reads.
pub fn read_declared_contract_with_admission(
    network: EvmNetwork,
    endpoint: &EvmRpcUrl,
    call: &DeclaredContractCall,
    caller: Option<&str>,
    block: &ContractBlock,
    admit: &impl Fn() -> bool,
) -> Result<ContractReadObservation<Value, String>, ContractError> {
    let rpc = RpcClient::new(endpoint).map_err(|_| ContractError::ProviderUnavailable)?;
    call.verify_calldata(call.calldata())?;
    let (context, caller, result) = read_raw(
        &AdmittedRpc { rpc: &rpc, admit },
        network,
        call.target(),
        call.calldata(),
        caller,
        block,
    )?;
    let outcome = match result {
        RpcCallOutcome::Succeeded(data) => ContractReadOutcome::Returned {
            values: call.decode_result(&data)?,
            data,
        },
        RpcCallOutcome::Reverted { code, data } => ContractReadOutcome::Reverted {
            code,
            interpretation: super::revert::decode(data.as_deref()),
            data,
        },
    };
    Ok(ContractReadObservation {
        context,
        caller,
        function: call.signature().into(),
        arguments: call.arguments().to_vec(),
        calldata: call.calldata().into(),
        output_types: call.outputs().to_vec(),
        value_wei: "0".into(),
        gas_limit: 1_000_000,
        outcome,
    })
}

fn read_raw(
    rpc: &impl ReadRpc,
    network: EvmNetwork,
    target: &str,
    calldata: &str,
    caller: Option<&str>,
    block: &ContractBlock,
) -> Result<(ContractCodeObservation, String, RpcCallOutcome), ContractError> {
    let caller =
        PublicEvmAddress::new(caller.unwrap_or("0x0000000000000000000000000000000000000000"))
            .map_err(|_| ContractError::InvalidAddress)?;
    let context = code_at(rpc, network, target, block)?;
    let result = rpc.call(json!([{
        "from": caller.as_str(), "to": target, "data": calldata,
        "value": "0x0", "gas": "0xf4240"
    }, {"blockHash":context.block_hash,"requireCanonical":true}]))?;
    check_canonical(rpc, &context)?;
    Ok((context, caller.as_str().into(), result))
}

fn code_at(
    rpc: &impl ReadRpc,
    network: EvmNetwork,
    target: &str,
    block: &ContractBlock,
) -> Result<ContractCodeObservation, ContractError> {
    let chain = rpc.value("eth_chainId", json!([]))?;
    network
        .validate_rpc_chain_id(chain.as_str().ok_or(ContractError::MalformedData)?)
        .map_err(|_| ContractError::WrongChain)?;
    let header = match block {
        ContractBlock::Latest => rpc.value("eth_getBlockByNumber", json!(["latest", false]))?,
        ContractBlock::Hash(hash) if valid_hex(hash, 32) => {
            rpc.value("eth_getBlockByHash", json!([hash, false]))?
        }
        ContractBlock::Hash(_) => return Err(ContractError::MalformedData),
    };
    let (number, hash) = header_identity(&header)?;
    if let ContractBlock::Hash(expected) = block
        && !hash.eq_ignore_ascii_case(expected)
    {
        return Err(ContractError::NonCanonicalBlock);
    }
    let code = rpc.value(
        "eth_getCode",
        json!([target, {"blockHash": hash, "requireCanonical": true}]),
    )?;
    let code = code.as_str().ok_or(ContractError::MalformedData)?;
    let bytes = decode_code(code)?;
    Ok(ContractCodeObservation {
        chain_id: network.descriptor().chain_id(),
        target: target.into(),
        block_number: number.to_decimal(),
        block_hash: hash,
        runtime_code_hash: hex_bytes(&Keccak256::digest(&bytes)),
        runtime_code_bytes: bytes.len(),
        source_qualified: false,
        writes_available: false,
        canonical_at_observation: true,
    })
}

fn header_identity(header: &Value) -> Result<(Uint256, String), ContractError> {
    let number = header
        .get("number")
        .and_then(Value::as_str)
        .ok_or(ContractError::NonCanonicalBlock)?;
    let number = Uint256::from_quantity(number).map_err(|_| ContractError::MalformedData)?;
    let hash = header
        .get("hash")
        .and_then(Value::as_str)
        .filter(|hash| valid_hex(hash, 32))
        .ok_or(ContractError::NonCanonicalBlock)?;
    Ok((number, hash.to_ascii_lowercase()))
}

fn check_canonical(
    rpc: &impl ReadRpc,
    context: &ContractCodeObservation,
) -> Result<(), ContractError> {
    let number =
        Uint256::from_decimal(&context.block_number).map_err(|_| ContractError::MalformedData)?;
    let header = rpc.value("eth_getBlockByNumber", json!([number.to_quantity(), false]))?;
    let (actual_number, actual_hash) = header_identity(&header)?;
    if number != actual_number || actual_hash != context.block_hash {
        return Err(ContractError::NonCanonicalBlock);
    }
    Ok(())
}

fn decode_code(code: &str) -> Result<Vec<u8>, ContractError> {
    if code == "0x" {
        return Err(ContractError::NoCode);
    }
    let hex = code
        .strip_prefix("0x")
        .ok_or(ContractError::MalformedData)?;
    if hex.len() > 24_576 * 2 || !valid_hex(code, hex.len() / 2) {
        return Err(ContractError::MalformedData);
    }
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).map_err(|_| ContractError::MalformedData)?;
            u8::from_str_radix(text, 16).map_err(|_| ContractError::MalformedData)
        })
        .collect()
}

#[cfg(test)]
#[path = "observation_tests.rs"]
mod tests;
