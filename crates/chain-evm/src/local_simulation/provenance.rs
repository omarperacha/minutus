//! Historical header lookup/canonicality for independently verified deployment proofs.
use crate::{EvmNetwork, EvmRpcUrl, rpc::RpcClient};
use alloy_consensus::Header;
use alloy_primitives::B256;
use evm_simulation::{SimulationControl, SimulationError};
use serde_json::json;
use std::{str::FromStr, time::Duration};

pub(crate) fn latest_header(
    endpoint: &EvmRpcUrl,
    network: EvmNetwork,
    control: &SimulationControl<'_>,
) -> Result<Header, SimulationError> {
    let rpc = RpcClient::new(endpoint).map_err(|_| SimulationError::StateUnavailable)?;
    let value = rpc
        .value_result_with_timeout(
            "eth_getBlockByNumber",
            &json!(["latest", false]),
            control.remaining()?.min(Duration::from_secs(1)),
        )
        .map_err(|_| SimulationError::StateUnavailable)?;
    control.remaining()?;
    let expected = value
        .get("hash")
        .and_then(serde_json::Value::as_str)
        .ok_or(SimulationError::InconsistentState)?;
    let header: Header =
        serde_json::from_value(value.clone()).map_err(|_| SimulationError::StateUnavailable)?;
    if format!("{:#x}", header.hash_slow()) != expected {
        return Err(SimulationError::InconsistentState);
    }
    canonical_header(endpoint, network, expected, control)
}

pub(crate) fn canonical_header(
    endpoint: &EvmRpcUrl,
    network: EvmNetwork,
    hash: &str,
    control: &SimulationControl<'_>,
) -> Result<Header, SimulationError> {
    let expected = B256::from_str(hash).map_err(|_| SimulationError::InvalidInput)?;
    let rpc = RpcClient::new(endpoint).map_err(|_| SimulationError::StateUnavailable)?;
    let call = |method, params| {
        let value = rpc
            .value_result_with_timeout(
                method,
                &params,
                control.remaining()?.min(Duration::from_secs(1)),
            )
            .map_err(|_| SimulationError::StateUnavailable)?;
        control.remaining()?;
        Ok::<_, SimulationError>(value)
    };
    let chain = call("eth_chainId", json!([]))?;
    network
        .validate_rpc_chain_id(chain.as_str().ok_or(SimulationError::InconsistentState)?)
        .map_err(|_| SimulationError::InconsistentState)?;
    let value = call("eth_getBlockByHash", json!([hash, false]))?;
    if value
        .get("hash")
        .and_then(serde_json::Value::as_str)
        .and_then(|v| B256::from_str(v).ok())
        != Some(expected)
    {
        return Err(SimulationError::InconsistentState);
    }
    let header: Header =
        serde_json::from_value(value).map_err(|_| SimulationError::StateUnavailable)?;
    if header.hash_slow() != expected {
        return Err(SimulationError::InconsistentState);
    }
    let again = call(
        "eth_getBlockByNumber",
        json!([format!("0x{:x}", header.number), false]),
    )?;
    let repeated: Header =
        serde_json::from_value(again).map_err(|_| SimulationError::StateUnavailable)?;
    if repeated.hash_slow() != expected {
        return Err(SimulationError::InconsistentState);
    }
    Ok(header)
}
