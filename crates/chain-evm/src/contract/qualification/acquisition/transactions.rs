//! Maintained transaction envelope codecs reconstruct exact public block leaves.
use super::{MAX_RAW, MAX_TRANSACTIONS, QualificationError, SimulationError, hash, quantity};
use crate::EvmNetwork;
use alloy_consensus::TxEnvelope;
use alloy_eips::eip2718::Encodable2718;
use alloy_primitives::{B256, keccak256};
use serde_json::Value;

pub(super) fn block_transactions(
    network: EvmNetwork,
    block: &Value,
    block_hash: B256,
    number: u64,
) -> Result<Vec<Vec<u8>>, QualificationError> {
    let transactions = block
        .get("transactions")
        .and_then(Value::as_array)
        .filter(|transactions| !transactions.is_empty() && transactions.len() <= MAX_TRANSACTIONS)
        .ok_or(SimulationError::Limit)?;
    transactions
        .iter()
        .enumerate()
        .map(|(index, transaction)| {
            if hash(transaction, "blockHash")? != block_hash
                || quantity(transaction, "blockNumber")? != number
                || quantity(transaction, "transactionIndex")?
                    != u64::try_from(index).map_err(|_| SimulationError::Limit)?
            {
                return Err(QualificationError::SnapshotMismatch);
            }
            encode(network, transaction)
        })
        .collect()
}
pub(super) fn encode(network: EvmNetwork, value: &Value) -> Result<Vec<u8>, QualificationError> {
    let raw = if network.is_base() {
        let envelope: base_consensus::BaseTxEnvelope = serde_json::from_value(value.clone())
            .map_err(|_| QualificationError::SnapshotMismatch)?;
        if envelope.encode_2718_len() > MAX_RAW {
            return Err(SimulationError::Limit.into());
        }
        envelope.encoded_2718()
    } else {
        let envelope: TxEnvelope = serde_json::from_value(value.clone())
            .map_err(|_| QualificationError::SnapshotMismatch)?;
        if envelope.encode_2718_len() > MAX_RAW {
            return Err(SimulationError::Limit.into());
        }
        envelope.encoded_2718()
    };
    if raw.is_empty() || raw.len() > MAX_RAW || keccak256(&raw) != hash(value, "hash")? {
        return Err(QualificationError::SnapshotMismatch);
    }
    Ok(raw)
}
