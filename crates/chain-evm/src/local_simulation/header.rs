use super::proof;
use alloy_consensus::Header;
use alloy_primitives::B256;
use evm_simulation::{SimulationBlock, SimulationError};
use serde_json::Value;

pub(super) fn decode(
    value: Value,
    expected: Option<B256>,
) -> Result<(Header, SimulationBlock), SimulationError> {
    let hash = proof::hash(proof::text(&value, "hash")?)?;
    let header: Header =
        serde_json::from_value(value).map_err(|_| SimulationError::StateUnavailable)?;
    if expected.is_some_and(|expected| expected != hash) || header.hash_slow() != hash {
        return Err(SimulationError::InconsistentState);
    }
    let block = SimulationBlock {
        hash: format!("{hash:#x}"),
        state_root: format!("{:#x}", header.state_root),
        number: header.number,
        timestamp: header.timestamp,
        beneficiary: format!("{:#x}", header.beneficiary),
        gas_limit: header.gas_limit,
        base_fee: header
            .base_fee_per_gas
            .ok_or(SimulationError::UnsupportedRules)?,
        prevrandao: format!("{:#x}", header.mix_hash),
        slot_number: header.slot_number,
        block_access_list_hash: header
            .block_access_list_hash
            .map(|hash| format!("{hash:#x}")),
        excess_blob_gas: header
            .excess_blob_gas
            .ok_or(SimulationError::UnsupportedRules)?,
    };
    Ok((header, block))
}
