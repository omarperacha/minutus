//! Canonical body and recovered-signer correspondence, with no signing authority.
use super::error::GenericError;
use crate::{
    DecodedEip1559Transaction,
    contract::UnsignedContractIntent,
    signing_payload::{CandidateFields, decode_eip1559, encode_candidate},
};
use alloy_consensus::{SignableTransaction, Signed, TxEip1559};
use alloy_primitives::{Address, U256, keccak256};
use evm_simulation::SimulationRequest;

pub(super) fn prepare(
    intent: &UnsignedContractIntent,
    request: &SimulationRequest,
) -> Result<(Vec<u8>, DecodedEip1559Transaction), GenericError> {
    let tx = &request.transaction;
    let value = U256::from_str_radix(
        tx.value
            .strip_prefix("0x")
            .ok_or(GenericError::SnapshotMismatch)?,
        16,
    )
    .map_err(|_| GenericError::SnapshotMismatch)?;
    if request.network.chain_id() != intent.network().descriptor().chain_id()
        || tx.caller != intent.account()
        || tx.target != intent.call().target()
        || tx.calldata != intent.call().calldata()
        || value.to_string() != intent.value_wei()
        || (intent.network().is_base() && value != U256::ZERO)
    {
        return Err(GenericError::SnapshotMismatch);
    }
    let payload = encode_candidate(CandidateFields {
        chain_id: request.network.chain_id(),
        nonce: &tx.nonce.to_string(),
        priority_fee: &tx.priority_fee_per_gas.to_string(),
        max_fee: &tx.max_fee_per_gas.to_string(),
        gas_limit: &tx.gas_limit.to_string(),
        destination: &tx.target,
        value: intent.value_wei(),
        data: &tx.calldata,
    })
    .map_err(|_| GenericError::SnapshotMismatch)?;
    let decoded =
        decode_eip1559(intent.account(), &payload).map_err(|_| GenericError::SnapshotMismatch)?;
    if decoded.chain_id() != request.network.chain_id()
        || decoded.nonce() != tx.nonce.to_string()
        || decoded.destination().to_ascii_lowercase() != tx.target
        || decoded.value_wei() != intent.value_wei()
        || decoded.data() != tx.calldata
        || decoded.gas_limit() != tx.gas_limit.to_string()
        || decoded.max_fee_per_gas_wei() != tx.max_fee_per_gas.to_string()
        || decoded.max_priority_fee_per_gas_wei() != tx.priority_fee_per_gas.to_string()
    {
        return Err(GenericError::SnapshotMismatch);
    }
    Ok((payload, decoded))
}

pub(super) fn signed_hash(
    payload: &[u8],
    account: &str,
    bytes: &[u8],
) -> Result<String, GenericError> {
    if bytes.len() > 8512 {
        return Err(GenericError::ExecutionRejected);
    }
    let mut remaining = bytes;
    let signed = Signed::<TxEip1559>::eip2718_decode(&mut remaining)
        .map_err(|_| GenericError::ExecutionRejected)?;
    let mut canonical = Vec::new();
    signed.eip2718_encode(&mut canonical);
    let address: Address = account
        .parse()
        .map_err(|_| GenericError::ExecutionRejected)?;
    if !remaining.is_empty()
        || canonical != bytes
        || signed.signature().normalize_s().is_some()
        || signed.tx().encoded_for_signing() != payload
        || signed
            .recover_signer()
            .map_err(|_| GenericError::ExecutionRejected)?
            != address
    {
        return Err(GenericError::ExecutionRejected);
    }
    Ok(format!("{:#x}", keccak256(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{B256, Signature, TxKind, U256, hex};
    #[test]
    fn exact_signed_body_recovered_signer_and_low_s_are_required()
    -> Result<(), Box<dyn std::error::Error>> {
        let tx = TxEip1559 {
            chain_id: 8453,
            nonce: 7,
            gas_limit: 100_000,
            max_fee_per_gas: 10,
            max_priority_fee_per_gas: 1,
            to: TxKind::Call(Address::repeat_byte(0x22)),
            ..Default::default()
        };
        let payload = tx.encoded_for_signing();
        let r = B256::from(hex!(
            "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
        ));
        let signature =
            Signature::from_scalars_and_parity(r, B256::from(U256::ONE.to_be_bytes::<32>()), false);
        let signed = Signed::new_unhashed(tx.clone(), signature);
        let caller = format!("{:#x}", signed.recover_signer()?);
        let mut bytes = Vec::new();
        signed.eip2718_encode(&mut bytes);
        assert_eq!(
            signed_hash(&payload, &caller, &bytes)?,
            format!("{:#x}", keccak256(&bytes))
        );
        assert!(signed_hash(&payload, &format!("{:#x}", Address::ZERO), &bytes).is_err());
        let mut changed = tx.clone();
        changed.nonce += 1;
        assert!(signed_hash(&changed.encoded_for_signing(), &caller, &bytes).is_err());
        let mut trailing = bytes;
        trailing.push(0);
        assert!(signed_hash(&payload, &caller, &trailing).is_err());
        let high = Signature::from_scalars_and_parity(
            r,
            B256::from(hex!(
                "fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364140"
            )),
            true,
        );
        let signed = Signed::new_unhashed(tx, high);
        let mut bytes = Vec::new();
        signed.eip2718_encode(&mut bytes);
        assert!(signed_hash(&payload, &caller, &bytes).is_err());
        Ok(())
    }
}
