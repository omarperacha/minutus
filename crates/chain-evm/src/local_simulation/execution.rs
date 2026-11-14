use crate::{
    EvmRpcUrl,
    contract::{ContractBlock, UnsignedContractIntent},
};
use evm_simulation::{
    SimulationControl, SimulationError, SimulationObservation, SimulationRequest,
    SimulationStateSource, StateQuery, TrustedSimulationWorker, simulate_in_worker,
    simulate_with_trusted_worker,
};
use std::{path::Path, sync::atomic::AtomicBool};

use super::{ProvedSimulation, SimulationFeeFacts};
/// Runs a trusted host-selected worker with current canonical proof-checked state.
/// No keys/unlock, transaction send or hosted simulation fallback is available.
/// This developer API is not yet an advertised or packaged chat capability.
/// RPC sees IP/account/code/storage/ancestor queries, even though execution is local.
///
/// # Errors
/// Rejects unsupported forks, absent proofs, inconsistent state, cancellation,
/// limits, validation failures or helper policy failure. Output never qualifies
/// arbitrary effects or source/proxy/ABI meaning, nor grants signing authority.
pub fn simulate_contract_with_worker(
    program: &Path,
    endpoint: &EvmRpcUrl,
    intent: &UnsignedContractIntent,
    block: &ContractBlock,
    fees: SimulationFeeFacts,
    cancelled: &AtomicBool,
) -> Result<SimulationObservation, SimulationError> {
    let control = SimulationControl::start(cancelled);
    Ok(run_with(
        endpoint,
        intent,
        block,
        fees,
        &control,
        &[],
        |request, source, control| simulate_in_worker(program, request, source, control),
    )?
    .into_observation())
}

pub(crate) fn run_proved(
    worker: &TrustedSimulationWorker,
    endpoint: &EvmRpcUrl,
    intent: &UnsignedContractIntent,
    block: &ContractBlock,
    fees: SimulationFeeFacts,
    control: &SimulationControl<'_>,
    preload: &[StateQuery],
) -> Result<ProvedSimulation, SimulationError> {
    run_with(
        endpoint,
        intent,
        block,
        fees,
        control,
        preload,
        |request, source, control| simulate_with_trusted_worker(worker, request, source, control),
    )
}
fn run_with(
    endpoint: &EvmRpcUrl,
    intent: &UnsignedContractIntent,
    block: &ContractBlock,
    fees: SimulationFeeFacts,
    control: &SimulationControl<'_>,
    preload: &[StateQuery],
    execute: impl FnOnce(
        &SimulationRequest,
        &mut super::source::StateSource<crate::rpc::RpcClient<'_>>,
        &SimulationControl<'_>,
    ) -> Result<SimulationObservation, SimulationError>,
) -> Result<ProvedSimulation, SimulationError> {
    let (request, mut source) = super::context::prepare(endpoint, intent, block, fees, control)?;
    for address in [&request.transaction.target, &request.block.beneficiary] {
        source.query(
            &StateQuery::Account {
                address: address.clone(),
            },
            control,
        )?;
    }
    for query in preload {
        source.query(query, control)?;
    }
    let observation = execute(&request, &mut source, control)?;
    source.canonical(control)?;
    Ok(ProvedSimulation::new(
        request,
        observation,
        source.into_proved_state(),
    ))
}
