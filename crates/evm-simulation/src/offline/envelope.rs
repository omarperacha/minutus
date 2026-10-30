use super::{MAX_ENVELOPE, snapshot::FrozenBaseRequest};
use crate::{SimulationError, admission};
use alloy_consensus::{Signed, TxEip1559};
use alloy_primitives::{Bytes, TxKind, keccak256};

// This is broadcast authority. No Debug/Serialize/Clone or raw public accessor.
pub(super) struct Envelope {
    pub(super) bytes: Bytes,
    pub(super) hash: String,
}
impl Envelope {
    pub(super) fn decode(
        snapshot: &FrozenBaseRequest,
        bytes: &[u8],
    ) -> Result<Self, SimulationError> {
        if bytes.len() > MAX_ENVELOPE || bytes.first() != Some(&2) {
            return Err(SimulationError::InvalidInput);
        }
        let mut remaining = bytes;
        let signed = Signed::<TxEip1559>::eip2718_decode(&mut remaining)
            .map_err(|_| SimulationError::InvalidInput)?;
        if !remaining.is_empty() || signed.signature().normalize_s().is_some() {
            return Err(SimulationError::InvalidInput);
        }
        let mut encoded = Vec::new();
        signed.eip2718_encode(&mut encoded);
        if encoded != bytes {
            return Err(SimulationError::InvalidInput);
        }
        let tx = signed.tx();
        let expected = &snapshot.request.transaction;
        if signed
            .recover_signer()
            .map_err(|_| SimulationError::InvalidInput)?
            != admission::address(&expected.caller)?
            || tx.chain_id != snapshot.request.network.chain_id()
            || tx.nonce != expected.nonce
            || tx.gas_limit != expected.gas_limit
            || tx.max_fee_per_gas != expected.max_fee_per_gas
            || tx.max_priority_fee_per_gas != expected.priority_fee_per_gas
            || tx.to != TxKind::Call(admission::address(&expected.target)?)
            || tx.value != admission::quantity(&expected.value)?
            || tx.input.as_ref() != admission::data(&expected.calldata, 8192)?
            || !tx.access_list.is_empty()
        {
            return Err(SimulationError::InconsistentState);
        }
        Ok(Self {
            bytes: Bytes::copy_from_slice(bytes),
            hash: format!("{:#x}", keccak256(bytes)),
        })
    }
}
