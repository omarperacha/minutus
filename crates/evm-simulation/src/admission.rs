use crate::contract::{MAX_GAS, SimulationError, SimulationNetwork, SimulationRequest, StateQuery};
use revm::primitives::{Address, B256, U256};
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Rules {
    Osaka,
    Amsterdam,
    BaseCobalt,
    BaseDenim,
}
// A maintenance deadline, not a prediction of the next network upgrade. Extend
// only after source/fork review; never default to "latest" EVM semantics.
const REVIEW_EXPIRES: u64 = 1_793_491_200; // 2026-11-01 UTC
pub(crate) fn rules(network: SimulationNetwork, timestamp: u64) -> Result<Rules, SimulationError> {
    if timestamp >= REVIEW_EXPIRES {
        return Err(SimulationError::UnsupportedRules);
    }
    match network {
        SimulationNetwork::EthereumMainnet if timestamp >= 1_764_798_551 => Ok(Rules::Osaka),
        SimulationNetwork::EthereumSepolia
            if (1_760_427_360..1_791_294_816).contains(&timestamp) =>
        {
            Ok(Rules::Osaka)
        }
        SimulationNetwork::EthereumSepolia if timestamp >= 1_791_294_816 => Ok(Rules::Amsterdam),
        SimulationNetwork::BaseMainnet if timestamp >= 1_790_791_200 => Ok(Rules::BaseCobalt),
        SimulationNetwork::BaseSepolia if timestamp >= 1_792_605_600 => Ok(Rules::BaseDenim),
        _ => Err(SimulationError::UnsupportedRules),
    }
}
pub(crate) fn validate(request: &SimulationRequest) -> Result<Rules, SimulationError> {
    let rules = rules(request.network, request.block.timestamp)?;
    let tx = &request.transaction;
    let block = &request.block;
    match rules {
        Rules::Amsterdam => {
            if block.slot_number.is_none() {
                return Err(SimulationError::UnsupportedRules);
            }
            hash(
                block
                    .block_access_list_hash
                    .as_deref()
                    .ok_or(SimulationError::UnsupportedRules)?,
            )?;
        }
        Rules::Osaka | Rules::BaseCobalt | Rules::BaseDenim => {
            if block.slot_number.is_some() || block.block_access_list_hash.is_some() {
                return Err(SimulationError::UnsupportedRules);
            }
        }
    }
    hash(&block.hash)?;
    hash(&block.state_root)?;
    hash(&block.prevrandao)?;
    address(&block.beneficiary)?;
    address(&tx.caller)?;
    if address(&tx.target)? == Address::ZERO
        || tx.gas_limit > MAX_GAS
        || tx.gas_limit < 21_000
        || tx.gas_limit > block.gas_limit
        || tx.priority_fee_per_gas > tx.max_fee_per_gas
        || tx.max_fee_per_gas < u128::from(block.base_fee)
    {
        return Err(SimulationError::InvalidInput);
    }
    quantity(&tx.value)?;
    data(&tx.calldata, 8192)?;
    Ok(rules)
}
pub(crate) fn validate_query(query: &StateQuery, block: u64) -> Result<(), SimulationError> {
    match query {
        StateQuery::Account { address: account } => {
            address(account)?;
        }
        StateQuery::Storage {
            address: account,
            slot,
        } => {
            address(account)?;
            hash(slot)?;
        }
        StateQuery::BlockHash { number } if *number >= block || block - number > 256 => {
            return Err(SimulationError::Protocol);
        }
        StateQuery::BlockHash { .. } => {}
    }
    Ok(())
}
pub(crate) fn address(value: &str) -> Result<Address, SimulationError> {
    if value.len() != 42 || !hex(value, 20) {
        return Err(SimulationError::InvalidInput);
    }
    Address::from_str(value).map_err(|_| SimulationError::InvalidInput)
}
pub(crate) fn hash(value: &str) -> Result<B256, SimulationError> {
    if !hex(value, 32) {
        return Err(SimulationError::InvalidInput);
    }
    B256::from_str(value).map_err(|_| SimulationError::InvalidInput)
}
pub(crate) fn quantity(value: &str) -> Result<U256, SimulationError> {
    let digits = value
        .strip_prefix("0x")
        .ok_or(SimulationError::InvalidInput)?;
    if digits.is_empty()
        || digits.len() > 64
        || (digits.len() > 1 && digits.starts_with('0'))
        || !digits.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(SimulationError::InvalidInput);
    }
    U256::from_str(value).map_err(|_| SimulationError::InvalidInput)
}
pub(crate) fn data(value: &str, max: usize) -> Result<Vec<u8>, SimulationError> {
    let digits = value
        .strip_prefix("0x")
        .ok_or(SimulationError::InvalidInput)?;
    if digits.len() > max * 2 || !hex(value, digits.len() / 2) {
        return Err(SimulationError::InvalidInput);
    }
    digits
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            std::str::from_utf8(pair)
                .ok()
                .and_then(|word| u8::from_str_radix(word, 16).ok())
                .ok_or(SimulationError::InvalidInput)
        })
        .collect()
}
fn hex(value: &str, length: usize) -> bool {
    value
        .strip_prefix("0x")
        .is_some_and(|word| word.len() == length * 2 && word.bytes().all(|b| b.is_ascii_hexdigit()))
}

pub(crate) fn blob_fraction(network: SimulationNetwork, timestamp: u64) -> u64 {
    let (bpo1, bpo2) = match network {
        SimulationNetwork::EthereumMainnet => (1_765_290_071, 1_767_747_671),
        SimulationNetwork::EthereumSepolia => (1_761_017_184, 1_761_607_008),
        // Base handler rejects missing exact envelopes before execution; these
        // are not used to claim its blob-opcode/fee context qualified.
        SimulationNetwork::BaseMainnet | SimulationNetwork::BaseSepolia => return 5_007_716,
    };
    if timestamp >= bpo2 {
        11_684_671
    } else if timestamp >= bpo1 {
        8_346_193
    } else {
        5_007_716
    }
}
