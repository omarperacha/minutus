//! Independent canonical unsigned body correspondence for both chain protocols.
use super::QualificationError;
use crate::{
    DecodedEip1559Transaction,
    contract::UnsignedContractIntent,
    signing_payload::{CandidateFields, decode_eip1559, encode_candidate},
};
use evm_simulation::SimulationRequest;

pub(super) fn prepare(
    intent: &UnsignedContractIntent,
    request: &SimulationRequest,
) -> Result<(Vec<u8>, DecodedEip1559Transaction), QualificationError> {
    let tx = &request.transaction;
    if request.network.chain_id() != intent.network().descriptor().chain_id()
        || tx.caller != intent.account()
        || tx.target != intent.call().target()
        || tx.calldata != intent.call().calldata()
        || tx.value != "0x0"
        || intent.value_wei() != "0"
    {
        return Err(QualificationError::SnapshotMismatch);
    }
    let payload = encode_candidate(CandidateFields {
        chain_id: request.network.chain_id(),
        nonce: &tx.nonce.to_string(),
        priority_fee: &tx.priority_fee_per_gas.to_string(),
        max_fee: &tx.max_fee_per_gas.to_string(),
        gas_limit: &tx.gas_limit.to_string(),
        destination: &tx.target,
        value: "0",
        data: &tx.calldata,
    })
    .map_err(|_| QualificationError::InvalidIntent)?;
    let decoded = decode_eip1559(intent.account(), &payload)
        .map_err(|_| QualificationError::InvalidIntent)?;
    if decoded.chain_id() != request.network.chain_id()
        || decoded.nonce() != tx.nonce.to_string()
        || decoded.destination().to_ascii_lowercase() != tx.target
        || decoded.value_wei() != "0"
        || decoded.data() != tx.calldata
        || decoded.gas_limit() != tx.gas_limit.to_string()
        || decoded.max_fee_per_gas_wei() != tx.max_fee_per_gas.to_string()
        || decoded.max_priority_fee_per_gas_wei() != tx.priority_fee_per_gas.to_string()
    {
        return Err(QualificationError::InvalidIntent);
    }
    Ok((payload, decoded))
}
