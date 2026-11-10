//! Bounded host acquisition of public included transaction/receipt MPT evidence.
//! Provider claims are checked against both complete canonical header roots.
use super::{DeploymentProof, QualificationError};
use crate::{
    EvmNetwork, EvmRpcUrl,
    rpc::{RpcClient, RpcError},
};
use alloy_consensus::{Header, Receipt, ReceiptWithBloom};
use alloy_primitives::{B256, Bytes, keccak256};
use alloy_trie::{
    HashBuilder, Nibbles,
    proof::{ProofRetainer, verify_proof},
};
use evm_simulation::{SimulationControl, SimulationError};
use serde_json::{Value, json};
use std::{str::FromStr, sync::atomic::AtomicBool, time::Duration};

mod receipts;
mod transactions;

const MAX_TRANSACTIONS: usize = 2048;
const MAX_RAW: usize = 256 * 1024;
const MAX_TOTAL: usize = 4 * 1024 * 1024;
struct Acquisition<'a, 'b> {
    rpc: RpcClient<'a>,
    control: &'b SimulationControl<'b>,
    bytes: usize,
}
impl Acquisition<'_, '_> {
    fn optional(
        &mut self,
        method: &str,
        params: &Value,
    ) -> Result<Option<Value>, QualificationError> {
        let remaining = MAX_TOTAL
            .checked_sub(self.bytes)
            .filter(|remaining| *remaining > 0)
            .ok_or(SimulationError::Limit)?;
        let (result, charged) = self
            .rpc
            .value_result_with_limit(
                method,
                params,
                self.control.remaining()?.min(Duration::from_secs(2)),
                u64::try_from(remaining).map_err(|_| SimulationError::Limit)?,
            )
            .map_err(|error| match error {
                RpcError::ResponseTooLarge => SimulationError::Limit,
                _ => SimulationError::StateUnavailable,
            })?;
        self.bytes = self
            .bytes
            .checked_add(charged)
            .ok_or(SimulationError::Limit)?;
        if self.bytes > MAX_TOTAL {
            return Err(SimulationError::Limit.into());
        }
        self.control.remaining()?;
        match result {
            Ok(value) => Ok(Some(value)),
            Err(RpcError::Rejected(-32601)) => Ok(None),
            Err(_) => Err(SimulationError::StateUnavailable.into()),
        }
    }
    fn call(&mut self, method: &str, params: &Value) -> Result<Value, QualificationError> {
        self.optional(method, params)?
            .ok_or_else(|| SimulationError::StateUnavailable.into())
    }
    fn canonical_block(
        &mut self,
        network: EvmNetwork,
        expected: B256,
        full: bool,
    ) -> Result<(Header, Value), QualificationError> {
        let chain = self.call("eth_chainId", &json!([]))?;
        network
            .validate_rpc_chain_id(chain.as_str().ok_or(QualificationError::SnapshotMismatch)?)
            .map_err(|_| QualificationError::SnapshotMismatch)?;
        let block = self.call(
            "eth_getBlockByHash",
            &json!([format!("{expected:#x}"), full]),
        )?;
        let header = checked_header(&block, expected)?;
        let repeated = self.call(
            "eth_getBlockByNumber",
            &json!([format!("0x{:x}", header.number), false]),
        )?;
        checked_header(&repeated, expected)?;
        Ok((header, block))
    }
}
fn checked_header(value: &Value, expected: B256) -> Result<Header, QualificationError> {
    let header: Header =
        serde_json::from_value(value.clone()).map_err(|_| QualificationError::SnapshotMismatch)?;
    if hash(value, "hash")? != expected || header.hash_slow() != expected {
        return Err(QualificationError::SnapshotMismatch);
    }
    Ok(header)
}
/// Acquires public inclusion evidence from the selected configured RPC after a
/// transaction is mined. No new provider or hosted simulator is contacted. Standard full-block transaction and batch-receipt RPCs supply bounded public
/// data; maintained Alloy/Base codecs reconstruct every trie leaf. At most2048
/// transactions and4MiB of aggregate RPC bodies are accepted in15 seconds.
///
/// # Errors
/// Pending/replaced/unknown transactions, unsupported envelopes, large blocks,
/// malformed receipts, root mismatches, reorgs, deadline or cancellation reject.
pub fn acquire_contract_transaction_proof(
    network: EvmNetwork,
    endpoint: &EvmRpcUrl,
    transaction_hash: &str,
    cancelled: &AtomicBool,
) -> Result<DeploymentProof, QualificationError> {
    let expected =
        B256::from_str(transaction_hash).map_err(|_| QualificationError::InvalidIntent)?;
    let control = SimulationControl::start(cancelled);
    let mut reader = Acquisition {
        rpc: RpcClient::new(endpoint).map_err(|_| SimulationError::StateUnavailable)?,
        control: &control,
        bytes: 0,
    };
    let own = reader.call("eth_getTransactionReceipt", &json!([transaction_hash]))?;
    if hash(&own, "transactionHash")? != expected {
        return Err(QualificationError::SnapshotMismatch);
    }
    let block_hash = hash(&own, "blockHash")?;
    let index = quantity(&own, "transactionIndex")?;
    let (header, block) = reader.canonical_block(network, block_hash, true)?;
    if quantity(&own, "blockNumber")? != header.number {
        return Err(QualificationError::SnapshotMismatch);
    }
    let transactions =
        transactions::block_transactions(network, &block, block_hash, header.number)?;
    let selected = usize::try_from(index).map_err(|_| QualificationError::SnapshotMismatch)?;
    if transactions.get(selected).map(keccak256) != Some(expected) {
        return Err(QualificationError::SnapshotMismatch);
    }
    let receipts = receipts::block_receipts(
        &mut reader,
        network,
        block_hash,
        header.number,
        &transactions,
        selected,
        &own,
    )?;
    let transaction_proof = trie_proof(&transactions, index, header.transactions_root)?;
    let receipt_proof = trie_proof(&receipts, index, header.receipts_root)?;
    reader.canonical_block(network, block_hash, false)?;
    let proof = DeploymentProof {
        block_hash: format!("{block_hash:#x}"),
        transaction_index: index,
        transaction: format!(
            "0x{}",
            alloy_primitives::hex::encode(&transactions[selected])
        ),
        transaction_proof,
        receipt: format!("0x{}", alloy_primitives::hex::encode(&receipts[selected])),
        receipt_proof,
    };
    // Match the narrow consumer's bounds before returning a partial certificate.
    if transactions[selected].len() > 8192 || receipts[selected].len() > 4096 {
        return Err(SimulationError::Limit.into());
    }
    Ok(proof)
}
fn hash(value: &Value, name: &str) -> Result<B256, QualificationError> {
    value
        .get(name)
        .and_then(Value::as_str)
        .and_then(|v| B256::from_str(v).ok())
        .ok_or(QualificationError::SnapshotMismatch)
}
fn quantity(value: &Value, name: &str) -> Result<u64, QualificationError> {
    value
        .get(name)
        .and_then(Value::as_str)
        .and_then(|v| v.strip_prefix("0x"))
        .filter(|digits| {
            !digits.is_empty()
                && digits.len() <= 16
                && !(digits.len() > 1 && digits.starts_with('0'))
                && digits
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        .and_then(|v| u64::from_str_radix(v, 16).ok())
        .ok_or(QualificationError::SnapshotMismatch)
}
fn receipt_bytes(
    network: EvmNetwork,
    raw: &[u8],
    value: &Value,
) -> Result<Vec<u8>, QualificationError> {
    let receipt: ReceiptWithBloom<Receipt> =
        serde_json::from_value(value.clone()).map_err(|_| QualificationError::SnapshotMismatch)?;
    if receipt.logs_bloom != receipt.receipt.bloom_slow() {
        return Err(QualificationError::SnapshotMismatch);
    }
    let first = *raw.first().ok_or(QualificationError::SnapshotMismatch)?;
    let kind = if first >= 0xc0 {
        0
    } else if first > 0 && first < 0x80 {
        first
    } else {
        return Err(QualificationError::SnapshotMismatch);
    };
    if quantity(value, "type")? != u64::from(kind) {
        return Err(QualificationError::SnapshotMismatch);
    }
    let mut bytes = if kind == 0 { vec![] } else { vec![kind] };
    if kind == 0x7e && network.is_base() {
        let deposit = base_consensus::DepositReceipt {
            inner: receipt.receipt,
            deposit_nonce: Some(quantity(value, "depositNonce")?),
            deposit_receipt_version: Some(quantity(value, "depositReceiptVersion")?),
        };
        bytes.extend(alloy_rlp::encode(ReceiptWithBloom {
            receipt: deposit,
            logs_bloom: receipt.logs_bloom,
        }));
    } else if kind == 0x79 && network.is_base() {
        // Pinned Base EIP8130 phase statuses are RPC-only metadata; maintained
        // native RLP commits only the standard receipt body and bloom.
        if value
            .get("depositNonce")
            .is_some_and(|value| !value.is_null())
            || value
                .get("depositReceiptVersion")
                .is_some_and(|value| !value.is_null())
        {
            return Err(QualificationError::SnapshotMismatch);
        }
        bytes.extend(alloy_rlp::encode(ReceiptWithBloom {
            receipt: base_consensus::Eip8130Receipt::new(receipt.receipt, vec![]),
            logs_bloom: receipt.logs_bloom,
        }));
    } else {
        if !matches!(kind, 0..=4)
            || value.get("depositNonce").is_some_and(|v| !v.is_null())
            || value
                .get("depositReceiptVersion")
                .is_some_and(|v| !v.is_null())
        {
            return Err(QualificationError::SnapshotMismatch);
        }
        bytes.extend(alloy_rlp::encode(receipt));
    }
    if bytes.len() > MAX_RAW {
        return Err(SimulationError::Limit.into());
    }
    Ok(bytes)
}
fn trie_proof(
    values: &[Vec<u8>],
    index: u64,
    expected: B256,
) -> Result<Vec<String>, QualificationError> {
    let key = Nibbles::unpack(alloy_rlp::encode(index));
    let mut leaves = values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            Ok((
                Nibbles::unpack(alloy_rlp::encode(
                    u64::try_from(index).map_err(|_| SimulationError::Limit)?,
                )),
                value,
            ))
        })
        .collect::<Result<Vec<_>, QualificationError>>()?;
    leaves.sort_by_key(|v| v.0);
    let mut builder = HashBuilder::default().with_proof_retainer(ProofRetainer::from_iter([key]));
    for (path, value) in leaves {
        builder.add_leaf(path, value);
    }
    if builder.root() != expected {
        return Err(QualificationError::SnapshotMismatch);
    }
    let nodes: Vec<Bytes> = builder
        .take_proof_nodes()
        .into_nodes_sorted()
        .into_iter()
        .map(|(_, v)| v)
        .collect();
    let value = values
        .get(usize::try_from(index).map_err(|_| SimulationError::Limit)?)
        .ok_or(QualificationError::SnapshotMismatch)?;
    verify_proof(expected, key, Some(value.clone()), &nodes)
        .map_err(|_| QualificationError::SnapshotMismatch)?;
    Ok(nodes.iter().map(|node| format!("{node:#x}")).collect())
}

#[cfg(test)]
mod tests;
