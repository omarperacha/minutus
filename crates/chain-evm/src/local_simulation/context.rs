//! Canonical exact-block request and proof broker composition shared by both modes.
use super::{SimulationFeeFacts, header, proof, source};
use crate::{
    EvmNetwork, EvmRpcUrl,
    contract::{ContractBlock, UnsignedContractIntent},
    rpc::RpcClient,
};
use alloy_primitives::U256;
use evm_simulation::{
    SimulationControl, SimulationError, SimulationNetwork, SimulationRequest,
    SimulationStateSource, SimulationTransaction, StateQuery, StateValue,
};
use serde_json::json;
use std::time::Duration;
pub(super) fn prepare<'a>(
    endpoint: &'a EvmRpcUrl,
    intent: &UnsignedContractIntent,
    block: &ContractBlock,
    fees: SimulationFeeFacts,
    control: &SimulationControl<'_>,
) -> Result<(SimulationRequest, source::StateSource<RpcClient<'a>>), SimulationError> {
    let rpc = RpcClient::new(endpoint).map_err(|_| SimulationError::StateUnavailable)?;
    let network = intent.network();
    let chain = rpc
        .value_result_with_timeout(
            "eth_chainId",
            &json!([]),
            control.remaining()?.min(Duration::from_secs(1)),
        )
        .map_err(|_| SimulationError::StateUnavailable)?;
    control.remaining()?;
    network
        .validate_rpc_chain_id(chain.as_str().ok_or(SimulationError::InconsistentState)?)
        .map_err(|_| SimulationError::InconsistentState)?;
    let (method, params, expected) = match block {
        ContractBlock::Latest => ("eth_getBlockByNumber", json!(["latest", false]), None),
        ContractBlock::Hash(hash) => (
            "eth_getBlockByHash",
            json!([hash, false]),
            Some(proof::hash(hash)?),
        ),
    };
    let value = rpc
        .value_result_with_timeout(
            method,
            &params,
            control.remaining()?.min(Duration::from_secs(1)),
        )
        .map_err(|_| SimulationError::StateUnavailable)?;
    let (header, block) = header::decode(value, expected)?;
    let hash = proof::hash(&block.hash)?;
    let mut source = source::StateSource::new(rpc, network, header, hash);
    source.canonical(control)?;
    let StateValue::Account { nonce, .. } = source.query(
        &StateQuery::Account {
            address: intent.account().into(),
        },
        control,
    )?
    else {
        return Err(SimulationError::Protocol);
    };
    let value =
        U256::from_str_radix(intent.value_wei(), 10).map_err(|_| SimulationError::InvalidInput)?;
    let request = SimulationRequest {
        network: match network {
            EvmNetwork::Mainnet => SimulationNetwork::BaseMainnet,
            EvmNetwork::Sepolia => SimulationNetwork::BaseSepolia,
            EvmNetwork::EthereumMainnet => SimulationNetwork::EthereumMainnet,
            EvmNetwork::EthereumSepolia => SimulationNetwork::EthereumSepolia,
        },
        block,
        transaction: SimulationTransaction {
            caller: intent.account().into(),
            target: intent.call().target().into(),
            value: format!("{value:#x}"),
            calldata: intent.call().calldata().into(),
            nonce,
            gas_limit: fees.gas_limit,
            max_fee_per_gas: fees.max_fee_per_gas,
            priority_fee_per_gas: fees.priority_fee_per_gas,
        },
    };
    Ok((request, source))
}
