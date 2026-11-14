//! Pure exact type-2 inclusion checks shared by task and receipt-only consumers.
//! Header canonicality belongs to the acquiring host; raw evidence is not a seal.
use super::{DeploymentProof, QualificationError, deployment, receipt_gas};
use crate::EvmNetwork;
use alloy_consensus::{Eip658Value, Header, Receipt, ReceiptWithBloom, Signed, TxEip1559};
use alloy_primitives::{B256, keccak256};
use alloy_rlp::Decodable;
use alloy_trie::{Nibbles, proof::verify_proof};
use std::str::FromStr;

pub(super) struct CheckedInclusion {
    pub(super) signed: Signed<TxEip1559>,
    pub(super) succeeded: bool,
}

#[cfg(debug_assertions)]
pub(crate) fn verify_public_success(
    network: EvmNetwork,
    header: &Header,
    proof: &DeploymentProof,
) -> Result<(), QualificationError> {
    let transaction = deployment::hex(&proof.transaction, 8192, false)?;
    let checked = verify(
        network,
        header,
        &format!("{:#x}", keccak256(transaction)),
        proof,
    )?;
    if !checked.succeeded || checked.signed.recover_signer().is_err() {
        return Err(QualificationError::SnapshotMismatch);
    }
    Ok(())
}

pub(super) fn verify(
    network: EvmNetwork,
    header: &Header,
    expected_hash: &str,
    proof: &DeploymentProof,
) -> Result<CheckedInclusion, QualificationError> {
    let reject = QualificationError::SnapshotMismatch;
    let transaction = deployment::hex(&proof.transaction, 8192, false)?;
    let expected = B256::from_str(expected_hash).map_err(|_| reject)?;
    if header.hash_slow() != B256::from_str(&proof.block_hash).map_err(|_| reject)?
        || keccak256(&transaction) != expected
    {
        return Err(reject);
    }
    let mut remaining = transaction.as_slice();
    let signed = Signed::<TxEip1559>::eip2718_decode(&mut remaining).map_err(|_| reject)?;
    let mut canonical = Vec::new();
    signed.eip2718_encode(&mut canonical);
    if !remaining.is_empty()
        || transaction.first() != Some(&2)
        || canonical != transaction
        || signed.tx().chain_id != network.descriptor().chain_id()
        || signed.signature().normalize_s().is_some()
    {
        return Err(reject);
    }
    let receipt = deployment::hex(&proof.receipt, 4096, false)?;
    let mut remaining = receipt.strip_prefix(&[2]).ok_or(reject)?;
    let decoded = ReceiptWithBloom::<Receipt>::decode(&mut remaining).map_err(|_| reject)?;
    let mut canonical = vec![2];
    canonical.extend(alloy_rlp::encode(&decoded));
    let succeeded = match decoded.receipt.status {
        Eip658Value::Eip658(value) => value,
        Eip658Value::PostState(_) => return Err(reject),
    };
    if !remaining.is_empty()
        || canonical != receipt
        || decoded.logs_bloom != decoded.receipt.bloom_slow()
    {
        return Err(reject);
    }
    receipt_gas::validate(network, header, decoded.receipt.cumulative_gas_used)?;
    let key = Nibbles::unpack(alloy_rlp::encode(proof.transaction_index));
    verify_proof(
        header.transactions_root,
        key,
        Some(transaction),
        &deployment::proof_nodes(&proof.transaction_proof)?,
    )
    .map_err(|_| reject)?;
    verify_proof(
        header.receipts_root,
        key,
        Some(receipt),
        &deployment::proof_nodes(&proof.receipt_proof)?,
    )
    .map_err(|_| reject)?;
    Ok(CheckedInclusion { signed, succeeded })
}
