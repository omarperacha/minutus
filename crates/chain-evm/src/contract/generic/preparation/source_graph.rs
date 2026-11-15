//! Source identities share one original exact-root proof/cache owner.
use super::{
    GenericContractIdentity, GenericError, GenericSourceProvider, account_controls, proxy,
    state_error,
};
use crate::{
    EvmNetwork,
    local_simulation::{ProvedReader, ProvedState},
};
use alloy_primitives::{Address, U256};
use evm_simulation::SimulationControl;
use std::{collections::BTreeSet, sync::atomic::AtomicBool};

pub(super) fn qualify(
    reader: &mut ProvedReader<'_>,
    network: EvmNetwork,
    entry: Address,
    business: &BTreeSet<Address>,
    sources: &mut GenericSourceProvider,
    cancelled: &AtomicBool,
    control: &SimulationControl<'_>,
) -> Result<GenericContractIdentity, GenericError> {
    for address in business {
        for query in account_controls(*address) {
            reader.query(&query, control).map_err(state_error)?;
        }
    }
    let initial = reader.snapshot(control).map_err(state_error)?;
    let implementations = implementations(&initial, business)?;
    let state = if implementations.is_empty() {
        initial
    } else {
        for implementation in implementations {
            for query in account_controls(implementation) {
                reader.query(&query, control).map_err(state_error)?;
            }
        }
        reader.snapshot(control).map_err(state_error)?
    };
    let mut artifacts = Vec::new();
    for address in state.account_addresses() {
        let code = state
            .account(address)
            .ok_or(GenericError::MissingState)?
            .code();
        if code.is_empty() || proxy::minimal_target(code).is_some() {
            continue;
        }
        artifacts.push((
            address,
            sources.artifact(network, &format!("{address:#x}"), code, cancelled)?,
        ));
    }
    // Compilation grants no canonicality. The original reader retains all
    // proof caches/counters and rechecks its exact anchor after reproduction.
    let state = reader.snapshot(control).map_err(state_error)?;
    let mut identities = business
        .iter()
        .map(|address| {
            GenericContractIdentity::from_source_state(network, *address, &state, &artifacts)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let position = business
        .iter()
        .position(|address| *address == entry)
        .ok_or(GenericError::InvalidAbi)?;
    let root = identities.remove(position);
    root.merge_sources(&identities.iter().collect::<Vec<_>>())
}

fn implementations(
    state: &ProvedState,
    business: &BTreeSet<Address>,
) -> Result<BTreeSet<Address>, GenericError> {
    let mut implementations = BTreeSet::new();
    for address in business {
        let account = state
            .account(*address)
            .filter(|value| value.exists() && !value.code().is_empty())
            .ok_or(GenericError::MissingState)?;
        if let Some(implementation) = proxy::minimal_target(account.code()) {
            if implementation == *address {
                return Err(GenericError::UnsupportedProxy);
            }
            implementations.insert(implementation);
        } else if state
            .slot(*address, proxy::IMPLEMENTATION)
            .is_some_and(|value| value != U256::ZERO)
        {
            // A write never acquires mutable implementation code as a fallback.
            return Err(GenericError::UnsupportedProxy);
        }
    }
    Ok(implementations)
}
