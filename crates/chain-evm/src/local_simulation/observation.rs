//! Proof-only reads share the simulation broker's exact header/account/slot checks.
use super::{ProvedState, canonical_header, source::StateSource};
use crate::{EvmNetwork, EvmRpcUrl, rpc::RpcClient};
use alloy_primitives::B256;
use evm_simulation::{SimulationControl, SimulationError, SimulationStateSource, StateQuery};

pub(crate) fn read_proved(
    endpoint: &EvmRpcUrl,
    network: EvmNetwork,
    block_hash: &str,
    queries: &[StateQuery],
    control: &SimulationControl<'_>,
) -> Result<(u64, ProvedState), SimulationError> {
    if queries.is_empty() || queries.len() > 32 {
        return Err(SimulationError::Limit);
    }
    let header = canonical_header(endpoint, network, block_hash, control)?;
    let number = header.number;
    let hash: B256 = header.hash_slow();
    let rpc = RpcClient::new(endpoint).map_err(|_| SimulationError::StateUnavailable)?;
    let mut source = StateSource::new(rpc, network, header, hash);
    for query in queries {
        source.query(query, control)?;
    }
    source.canonical(control)?;
    Ok((number, source.into_proved_state()))
}
