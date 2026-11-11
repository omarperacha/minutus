//! Generic candidates use host-owned maintained replay, never a child's claimed
//! transition. Discovery may request proved public state; the final replay is a
//! closed map with no network/broker, signature or wallet capability.
use super::{ProvedState, SimulationFeeFacts, context};
use crate::{
    EvmRpcUrl,
    contract::{ContractBlock, UnsignedContractIntent},
};
use alloy_primitives::{Address, U256};
use evm_simulation::{
    NativeTransfer, ReplayCall, SimulationControl, SimulationError, SimulationObservation,
    SimulationRequest, SimulationStateSource, StateQuery, StateValue, TrustedReplayExecution,
    TrustedSimulationWorkers, replay_discover, replay_frozen, simulate_with_trusted_worker,
};
use std::{str::FromStr, time::Instant};

/// Proof-sealed trusted transition. Private construction joins maintained host
/// execution with the chain owner's MPT/header/canonicality checks.
pub(crate) struct TrustedReplay {
    request: SimulationRequest,
    state: ProvedState,
    execution: TrustedReplayExecution,
    written: Vec<(Address, U256)>,
    prepared_at: Instant,
    base: Option<evm_simulation::offline::FrozenBaseRequest>,
    base_fee: Option<String>,
}
impl TrustedReplay {
    pub(crate) const fn request(&self) -> &SimulationRequest {
        &self.request
    }
    pub(crate) const fn state(&self) -> &ProvedState {
        &self.state
    }
    pub(crate) fn observation(&self) -> &SimulationObservation {
        self.execution.observation()
    }
    pub(crate) fn keccak_preimages(&self) -> &[Vec<u8>] {
        self.execution.keccak_preimages()
    }
    pub(crate) fn written_slots(&self) -> &[(Address, U256)] {
        &self.written
    }
    pub(crate) fn native_transfers(&self) -> &[NativeTransfer] {
        self.execution.native_transfers()
    }
    pub(crate) fn calls(&self) -> &[ReplayCall] {
        self.execution.calls()
    }
    pub(crate) fn is_base_material(&self) -> bool {
        self.base.is_some()
    }
    pub(crate) fn base_fee_ceiling(&self) -> Option<&str> {
        self.base_fee.as_deref()
    }
    pub(crate) const fn prepared_at(&self) -> Instant {
        self.prepared_at
    }
    pub(crate) fn validate_base_signed(
        self,
        worker: &evm_simulation::TrustedSimulationWorker,
        signed: &[u8],
        control: &SimulationControl<'_>,
    ) -> Result<evm_simulation::offline::ValidatedOfflineExecution, SimulationError> {
        let expected = evm_simulation::offline::OfflineMaterialExpectation::from_trusted_replay(
            &self.execution,
        )?;
        let frozen = self.base.ok_or(SimulationError::BaseEnvelopeRequired)?;
        evm_simulation::offline::execute_base_offline(worker, frozen, signed, control)?
            .validate(&expected)
    }
}

pub(crate) fn replay_proved(
    workers: &TrustedSimulationWorkers,
    endpoint: &EvmRpcUrl,
    intent: &UnsignedContractIntent,
    block: &ContractBlock,
    fees: SimulationFeeFacts,
    preload: &[StateQuery],
    control: &SimulationControl<'_>,
) -> Result<TrustedReplay, SimulationError> {
    let prepared_at = Instant::now();
    let (request, source) = context::prepare(endpoint, intent, block, fees, control)?;
    replay_source(workers, request, source, preload, control, prepared_at)
}

/// Consumes the original proof/cache/header owner; no bare state can seed replay.
pub(crate) fn replay_prepared_reader(
    workers: &TrustedSimulationWorkers,
    reader: super::ProvedReader<'_>,
    intent: &UnsignedContractIntent,
    fees: SimulationFeeFacts,
    preload: &[StateQuery],
    control: &SimulationControl<'_>,
) -> Result<TrustedReplay, SimulationError> {
    let prepared_at = Instant::now();
    let (request, source) = context::from_reader(reader, intent, fees, control)?;
    replay_source(workers, request, source, preload, control, prepared_at)
}

fn replay_source(
    workers: &TrustedSimulationWorkers,
    request: SimulationRequest,
    mut source: super::source::StateSource<crate::rpc::RpcClient<'_>>,
    preload: &[StateQuery],
    control: &SimulationControl<'_>,
    prepared_at: Instant,
) -> Result<TrustedReplay, SimulationError> {
    for query in preload {
        source.query(query, control)?;
    }
    // Discovery output is intentionally discarded. A lying/compromised worker
    // can at most omit required state and cause the independent replay to reject.
    let base_network = matches!(
        request.network,
        evm_simulation::SimulationNetwork::BaseMainnet
            | evm_simulation::SimulationNetwork::BaseSepolia
    );
    if base_network {
        for query in evm_simulation::offline::base_native_manifest(&request)? {
            source.query(&query, control)?;
        }
        let _ = replay_discover(&request, &mut source, control)?;
    } else {
        let _ = simulate_with_trusted_worker(workers.online(), &request, &mut source, control)?;
    }
    preload_dispatch_controls(&mut source, control)?;
    let facts = source.frozen_facts();
    let execution = replay_frozen(&request, &facts, control)?;
    let (base, base_fee) = if base_network {
        if !execution.is_base_application() {
            return Err(SimulationError::BaseEnvelopeRequired);
        }
        let queries = facts
            .iter()
            .map(|(query, _)| query.clone())
            .collect::<Vec<_>>();
        let snapshot = evm_simulation::offline::FrozenBaseRequest::prepare(
            request.clone(),
            &queries,
            &mut source,
            control,
        )?;
        let fee = snapshot.fee_ceiling()?.total_wei();
        if execution.fee_ceiling_wei() != Some(fee.as_str()) {
            return Err(SimulationError::InconsistentState);
        }
        (Some(snapshot), Some(fee))
    } else {
        (None, None)
    };
    source.canonical(control)?;
    control.remaining()?;
    let written = execution
        .written_slots()
        .iter()
        .map(|(address, slot)| {
            Ok((
                Address::from_str(address).map_err(|_| SimulationError::InconsistentState)?,
                U256::from_str(slot).map_err(|_| SimulationError::InconsistentState)?,
            ))
        })
        .collect::<Result<Vec<_>, SimulationError>>()?;
    Ok(TrustedReplay {
        request,
        state: source.into_proved_state(),
        execution,
        written,
        prepared_at,
        base,
        base_fee,
    })
}

fn preload_dispatch_controls<R: super::source::StateRpc>(
    source: &mut super::source::StateSource<R>,
    control: &SimulationControl<'_>,
) -> Result<(), SimulationError> {
    let code_accounts: Vec<_> = source
        .frozen_facts()
        .into_iter()
        .filter_map(|(query, value)| match (query, value) {
            (StateQuery::Account { address }, StateValue::Account { code, .. })
                if code != "0x" && !system(&address) =>
            {
                Some(address)
            }
            _ => None,
        })
        .collect();
    for address in code_accounts {
        for slot in CONTROL_SLOTS {
            let value = source.query(
                &StateQuery::Storage {
                    address: address.clone(),
                    slot: slot.into(),
                },
                control,
            )?;
            if slot == CONTROL_SLOTS[0]
                && let StateValue::Storage { value } = value
            {
                let word =
                    U256::from_str(&value).map_err(|_| SimulationError::InconsistentState)?;
                if word != U256::ZERO {
                    let bytes = word.to_be_bytes::<32>();
                    if bytes[..12].iter().any(|byte| *byte != 0) {
                        return Err(SimulationError::UnsupportedExecution);
                    }
                    let implementation = Address::from_slice(&bytes[12..]);
                    source.query(
                        &StateQuery::Account {
                            address: format!("{implementation:#x}"),
                        },
                        control,
                    )?;
                }
            }
        }
    }
    Ok(())
}
fn system(address: &str) -> bool {
    Address::from_str(address).is_ok_and(|value| {
        let bytes = value.as_slice();
        (bytes[..19].iter().all(|byte| *byte == 0) && bytes[19] <= 0x20)
            || (bytes[0] == 0x42 && bytes[1..19].iter().all(|byte| *byte == 0))
    })
}
const CONTROL_SLOTS: [&str; 3] = [
    "0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc",
    "0xb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103",
    "0xa3f0ad74e5423aebfd80d3ef4346578335a9a72aeaee59ff6cb3582b35133d50",
];

#[cfg(test)]
mod tests;
