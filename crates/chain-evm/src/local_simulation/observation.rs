//! Proof-only reads share the simulation broker's exact header/account/slot checks.
use super::{ProvedState, canonical_header, source::StateSource};
use crate::{EvmNetwork, EvmRpcUrl, rpc::RpcClient};
use alloy_primitives::B256;
use evm_simulation::{
    SimulationControl, SimulationError, SimulationStateSource, StateQuery, StateValue,
};

/// One exact-root proof session. Repeated typed-path reads reuse the same proved
/// account/code/slot cache; only a canonical finish releases sealed state.
pub(crate) struct ProvedReader<'a> {
    source: StateSource<RpcClient<'a>>,
}
impl<'a> ProvedReader<'a> {
    pub(crate) fn new(
        endpoint: &'a EvmRpcUrl,
        network: EvmNetwork,
        block_hash: &str,
        control: &SimulationControl<'_>,
    ) -> Result<Self, SimulationError> {
        let header = canonical_header(endpoint, network, block_hash, control)?;
        let hash: B256 = header.hash_slow();
        let rpc = RpcClient::new(endpoint).map_err(|_| SimulationError::StateUnavailable)?;
        Ok(Self {
            source: StateSource::new(rpc, network, header, hash),
        })
    }
    pub(crate) const fn block_number(&self) -> u64 {
        self.source.header.number
    }
    pub(crate) fn query(
        &mut self,
        query: &StateQuery,
        control: &SimulationControl<'_>,
    ) -> Result<StateValue, SimulationError> {
        self.source.query(query, control)
    }
    pub(crate) fn finish(
        mut self,
        control: &SimulationControl<'_>,
    ) -> Result<ProvedState, SimulationError> {
        self.source.canonical(control)?;
        Ok(self.source.into_proved_state())
    }
}

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
    let mut source = ProvedReader::new(endpoint, network, block_hash, control)?;
    let number = source.block_number();
    for query in queries {
        source.query(query, control)?;
    }
    Ok((number, source.finish(control)?))
}
