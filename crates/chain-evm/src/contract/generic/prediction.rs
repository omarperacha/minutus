//! Declared completion goals must be feasible in the trusted simulated poststate
//! before native approval. This is neither inclusion nor business causation.
use super::{
    GenericCondition, GenericContractCandidate, GenericContractIdentity, GenericError,
    completion::{condition_artifact, satisfies},
    layout,
};
use crate::{EvmRpcUrl, local_simulation};
use alloy_primitives::{Address, U256};
use evm_simulation::{SimulationControl, StateQuery};
use std::{collections::BTreeMap, str::FromStr, sync::atomic::AtomicBool};

/// Checks declared postconditions against the actual trusted replay's changes
/// over proof-checked state at that same canonical block. Missing ancillary
/// fields are independently read at the pinned block, never guessed as zero.
///
/// # Errors
/// Rejects source/cohort mismatch, known-unsatisfied predicted goals, missing
/// proof, cancellation, stale candidates and all fixed condition/read bounds.
pub fn verify_predicted_generic_conditions(
    endpoint: &EvmRpcUrl,
    candidate: &GenericContractCandidate,
    identities: &[&GenericContractIdentity],
    conditions: &[GenericCondition],
    cancelled: &AtomicBool,
) -> Result<(), GenericError> {
    let control = SimulationControl::start(cancelled);
    current(candidate, &control)?;
    if conditions.is_empty() {
        return Ok(());
    }
    if identities.is_empty() || identities.len() > 32 || conditions.len() > 16 {
        return Err(GenericError::Limit);
    }
    if !identities
        .iter()
        .any(|identity| identity.commitment() == candidate.identity().commitment())
    {
        return Err(GenericError::SourceMismatch);
    }
    let network = candidate.intent().network();
    let block = candidate.block_hash();
    let mut reader = local_simulation::ProvedReader::new(endpoint, network, block, &control)
        .map_err(|_| GenericError::MissingState)?;
    if reader.block_number() != candidate.block_number() {
        return Err(GenericError::SnapshotMismatch);
    }
    for identity in identities {
        if identity.network() != network {
            return Err(GenericError::SnapshotMismatch);
        }
        for query in identity.state_queries() {
            reader
                .query(&query, &control)
                .map_err(|_| GenericError::MissingState)?;
        }
    }
    let changed = changed_slots(candidate)?;
    let mut reads = BTreeMap::new();
    let mut satisfied = true;
    for condition in conditions {
        let address = Address::from_str(&condition.target).map_err(|_| GenericError::InvalidAbi)?;
        let artifact = condition_artifact(identities, address)?;
        let observed = layout::read_condition_path(
            artifact.storage_layout(),
            &condition.path,
            address,
            |query| {
                let StateQuery::Storage {
                    address: target,
                    slot,
                } = &query
                else {
                    return Err(GenericError::OpaqueEffects);
                };
                if Address::from_str(target).map_err(|_| GenericError::InvalidAbi)? != address {
                    return Err(GenericError::SnapshotMismatch);
                }
                let slot = U256::from_str(slot).map_err(|_| GenericError::InvalidAbi)?;
                if let Some(after) = changed.get(&(address, slot)) {
                    return Ok(*after);
                }
                if let Some(before) = candidate.replay().state().slot(address, slot) {
                    return Ok(before);
                }
                if let Some(value) = reads.get(&(address, slot)) {
                    return Ok(*value);
                }
                if reads.len() >= 128 {
                    return Err(GenericError::Limit);
                }
                let evm_simulation::StateValue::Storage { value } = reader
                    .query(&query, &control)
                    .map_err(|_| GenericError::MissingState)?
                else {
                    return Err(GenericError::MissingState);
                };
                let value = U256::from_str(&value).map_err(|_| GenericError::MissingState)?;
                reads.insert((address, slot), value);
                Ok(value)
            },
        )?;
        if !satisfies(condition, &observed.solidity_type, &observed.value)? {
            satisfied = false;
            break;
        }
    }
    let state = reader
        .finish(&control)
        .map_err(|_| GenericError::SnapshotMismatch)?;
    for identity in identities {
        identity.verify_state(&state)?;
    }
    current(candidate, &control)?;
    if !satisfied {
        return Err(GenericError::ConditionUnsatisfied);
    }
    Ok(())
}

fn changed_slots(
    candidate: &GenericContractCandidate,
) -> Result<BTreeMap<(Address, U256), U256>, GenericError> {
    let mut changed = BTreeMap::new();
    for change in &candidate.replay().observation().storage {
        let address =
            Address::from_str(&change.address).map_err(|_| GenericError::SnapshotMismatch)?;
        let slot = U256::from_str(&change.slot).map_err(|_| GenericError::SnapshotMismatch)?;
        let after = U256::from_str(&change.after).map_err(|_| GenericError::SnapshotMismatch)?;
        if changed.insert((address, slot), after).is_some() {
            return Err(GenericError::SnapshotMismatch);
        }
    }
    Ok(changed)
}

fn current(
    candidate: &GenericContractCandidate,
    control: &SimulationControl<'_>,
) -> Result<(), GenericError> {
    control.remaining().map_err(|_| GenericError::Expired)?;
    if !candidate.is_fresh() {
        return Err(GenericError::Expired);
    }
    Ok(())
}
