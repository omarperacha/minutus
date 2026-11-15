//! Only this pre-signature path has a trusted proof-source callback. Each missing
//! fact restarts the maintained engine from the unchanged exact snapshot.
use super::{MissingQuery, replay_recording};
use crate::{
    SimulationControl, SimulationError, SimulationRequest, SimulationStateSource, StateQuery,
    StateValue,
};
use std::sync::{Arc, Mutex};

/// Discovers a bounded closed read witness using the trusted host's proof broker.
/// No signed bytes are accepted. Values confer no proof authority: chain-evm
/// binds them to its state root before a separate frozen replay.
///
/// # Errors
/// Rejects proof-source failures, cancellation, limits and unsupported execution.
pub fn replay_discover(
    request: &SimulationRequest,
    source: &mut impl SimulationStateSource,
    control: &SimulationControl<'_>,
) -> Result<Vec<(StateQuery, StateValue)>, SimulationError> {
    let manifest = match request.network {
        crate::SimulationNetwork::BaseMainnet | crate::SimulationNetwork::BaseSepolia => {
            crate::offline::base_native_manifest(request)?
        }
        _ => [
            &request.transaction.caller,
            &request.transaction.target,
            &request.block.beneficiary,
        ]
        .into_iter()
        .map(|address| StateQuery::Account {
            address: address.clone(),
        })
        .collect(),
    };
    let mut facts = Vec::new();
    for query in manifest {
        acquire(request, query, &mut facts, source, control)?;
    }
    loop {
        control.remaining()?;
        let missing: MissingQuery = Arc::new(Mutex::new(None));
        match replay_recording(request, &facts, control, Some(Arc::clone(&missing))) {
            Ok(_) => return Ok(facts),
            Err(SimulationError::StateUnavailable) => {
                let query = missing
                    .lock()
                    .map_err(|_| SimulationError::Protocol)?
                    .take()
                    .ok_or(SimulationError::StateUnavailable)?;
                if facts.iter().any(|(previous, _)| previous == &query) {
                    return Err(SimulationError::InconsistentState);
                }
                acquire(request, query, &mut facts, source, control)?;
            }
            Err(error) => return Err(error),
        }
    }
}
fn acquire(
    request: &SimulationRequest,
    query: StateQuery,
    facts: &mut Vec<(StateQuery, StateValue)>,
    source: &mut impl SimulationStateSource,
    control: &SimulationControl<'_>,
) -> Result<(), SimulationError> {
    if facts.iter().any(|(previous, _)| previous == &query) {
        return Ok(());
    }
    if facts.len() >= crate::MAX_STATE_QUERIES as usize {
        return Err(SimulationError::Limit);
    }
    control.remaining()?;
    let value = source.query(&query, control)?;
    control.remaining()?;
    super::validate_fact(request, &query, &value)?;
    facts.push((query, value));
    if serde_json::to_vec(facts)
        .map_err(|_| SimulationError::Protocol)?
        .len()
        > crate::MAX_STATE_BYTES
    {
        return Err(SimulationError::Limit);
    }
    Ok(())
}
