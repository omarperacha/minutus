//! Bounded batch receipts, with fallback only for an explicit unsupported method.
use super::{Acquisition, QualificationError, SimulationError, hash, quantity, receipt_bytes};
use crate::EvmNetwork;
use alloy_primitives::{B256, keccak256};
use serde_json::{Value, json};

pub(super) fn block_receipts(
    reader: &mut Acquisition<'_, '_>,
    network: EvmNetwork,
    block_hash: B256,
    number: u64,
    transactions: &[Vec<u8>],
    selected: usize,
    own: &Value,
) -> Result<Vec<Vec<u8>>, QualificationError> {
    let batch = reader.optional("eth_getBlockReceipts", &json!([format!("{block_hash:#x}")]))?;
    let values = if let Some(batch) = batch {
        let batch = batch
            .as_array()
            .filter(|batch| batch.len() == transactions.len())
            .ok_or(QualificationError::SnapshotMismatch)?;
        let mut ordered = vec![None; transactions.len()];
        for receipt in batch {
            let index = usize::try_from(quantity(receipt, "transactionIndex")?)
                .map_err(|_| QualificationError::SnapshotMismatch)?;
            let slot = ordered
                .get_mut(index)
                .ok_or(QualificationError::SnapshotMismatch)?;
            if slot.replace(receipt.clone()).is_some() {
                return Err(QualificationError::SnapshotMismatch);
            }
        }
        ordered
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or(QualificationError::SnapshotMismatch)?
    } else {
        transactions
            .iter()
            .enumerate()
            .map(|(index, transaction)| {
                if index == selected {
                    Ok(own.clone())
                } else {
                    reader.call(
                        "eth_getTransactionReceipt",
                        &json!([format!("{:#x}", keccak256(transaction))]),
                    )
                }
            })
            .collect::<Result<Vec<_>, QualificationError>>()?
    };
    let receipts = values
        .iter()
        .zip(transactions)
        .enumerate()
        .map(|(index, (receipt, transaction))| {
            if hash(receipt, "transactionHash")? != keccak256(transaction)
                || hash(receipt, "blockHash")? != block_hash
                || quantity(receipt, "blockNumber")? != number
                || quantity(receipt, "transactionIndex")?
                    != u64::try_from(index).map_err(|_| SimulationError::Limit)?
            {
                return Err(QualificationError::SnapshotMismatch);
            }
            receipt_bytes(network, transaction, receipt)
        })
        .collect::<Result<Vec<_>, QualificationError>>()?;
    if receipts.get(selected) != Some(&receipt_bytes(network, &transactions[selected], own)?) {
        return Err(QualificationError::SnapshotMismatch);
    }
    Ok(receipts)
}
