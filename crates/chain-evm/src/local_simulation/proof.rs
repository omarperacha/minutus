//! EIP-1186 MPT proofs bind account/slot data to the pinned header state root.
//! Header identity remains provider-trusted, not consensus/light-client proof.
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use alloy_trie::{EMPTY_ROOT_HASH, Nibbles, TrieAccount, proof::verify_proof};
use evm_simulation::SimulationError;
use serde_json::Value;
use std::str::FromStr;

pub(super) fn account(
    root: B256,
    address: Address,
    value: &Value,
) -> Result<(bool, TrieAccount), SimulationError> {
    let observed = Address::from_str(text(value, "address")?)
        .map_err(|_| SimulationError::InconsistentState)?;
    if observed != address {
        return Err(SimulationError::InconsistentState);
    }
    let account = TrieAccount {
        nonce: quantity(text(value, "nonce")?)?
            .try_into()
            .map_err(|_| SimulationError::InconsistentState)?,
        balance: quantity(text(value, "balance")?)?,
        storage_root: hash(text(value, "storageHash")?)?,
        code_hash: hash(text(value, "codeHash")?)?,
    };
    let nodes = nodes(
        value
            .get("accountProof")
            .ok_or(SimulationError::StateUnavailable)?,
    )?;
    let key = Nibbles::unpack(keccak256(address));
    if verify_proof(root, key, Some(alloy_rlp::encode(account)), &nodes).is_ok() {
        return Ok((true, account));
    }
    verify_proof(root, key, None, &nodes).map_err(|_| SimulationError::InconsistentState)?;
    if account.nonce != 0
        || account.balance != U256::ZERO
        || account.storage_root != EMPTY_ROOT_HASH
        || account.code_hash != keccak256([])
    {
        return Err(SimulationError::InconsistentState);
    }
    Ok((false, account))
}

pub(super) fn storage(root: B256, key: U256, value: &Value) -> Result<U256, SimulationError> {
    let items = value
        .get("storageProof")
        .and_then(Value::as_array)
        .filter(|items| items.len() == 1)
        .ok_or(SimulationError::StateUnavailable)?;
    let entry = &items[0];
    let observed =
        U256::from_str(text(entry, "key")?).map_err(|_| SimulationError::InconsistentState)?;
    if observed != key {
        return Err(SimulationError::InconsistentState);
    }
    let value = quantity(text(entry, "value")?)?;
    let proof = nodes(
        entry
            .get("proof")
            .ok_or(SimulationError::StateUnavailable)?,
    )?;
    let expected = if value == U256::ZERO {
        None
    } else {
        Some(alloy_rlp::encode(value))
    };
    verify_proof(
        root,
        Nibbles::unpack(keccak256(key.to_be_bytes::<32>())),
        expected,
        &proof,
    )
    .map_err(|_| SimulationError::InconsistentState)?;
    Ok(value)
}
fn nodes(value: &Value) -> Result<Vec<Bytes>, SimulationError> {
    let list = value
        .as_array()
        .filter(|list| list.len() <= 64)
        .ok_or(SimulationError::Limit)?;
    let mut total = 0;
    list.iter()
        .map(|node| {
            let node = node.as_str().ok_or(SimulationError::InconsistentState)?;
            total += node.len();
            if total > 48 * 1024 {
                return Err(SimulationError::Limit);
            }
            Bytes::from_str(node).map_err(|_| SimulationError::InconsistentState)
        })
        .collect()
}
pub(super) fn text<'a>(value: &'a Value, name: &str) -> Result<&'a str, SimulationError> {
    value
        .get(name)
        .and_then(Value::as_str)
        .ok_or(SimulationError::StateUnavailable)
}
pub(super) fn hash(value: &str) -> Result<B256, SimulationError> {
    B256::from_str(value).map_err(|_| SimulationError::InconsistentState)
}
pub(super) fn quantity(value: &str) -> Result<U256, SimulationError> {
    let hex = value
        .strip_prefix("0x")
        .ok_or(SimulationError::InconsistentState)?;
    if hex.is_empty()
        || hex.len() > 64
        || (hex.len() > 1 && hex.starts_with('0'))
        || !hex.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(SimulationError::InconsistentState);
    }
    U256::from_str(value).map_err(|_| SimulationError::InconsistentState)
}
