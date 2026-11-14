//! Collected conditions stay private until the last canonical/freshness checks.
use super::{
    CandidateStateReader, GenericCondition, GenericConditionObservation, GenericContractIdentity,
    GenericError, Stage, VerifiedGenericConditions, commitment, condition_artifact, diagnostics,
    satisfies,
};
use crate::{
    EvmNetwork, EvmRpcUrl,
    contract::generic::{GenericLiveCohort, layout},
};
use alloy_primitives::{Address, U256};
use evm_simulation::{SimulationControl, StateQuery};
use std::{collections::BTreeMap, str::FromStr, sync::atomic::AtomicBool};

pub(super) struct PendingGenericConditions {
    cohort: GenericLiveCohort,
    source_commitments: Vec<String>,
    observations: Vec<GenericConditionObservation>,
    satisfied: bool,
}
impl PendingGenericConditions {
    pub(super) const fn block_number(&self) -> u64 {
        self.cohort.block_number()
    }
    pub(super) fn finalise(
        self,
        endpoint: &EvmRpcUrl,
        control: &SimulationControl<'_>,
    ) -> Result<VerifiedGenericConditions, GenericError> {
        self.cohort.check(endpoint, control)?;
        // These checks happen after the last canonical reads, including for a
        // proved false predicate. The original selection clock is not renewed.
        if !self.cohort.is_fresh() {
            return Err(GenericError::Expired);
        }
        if !self.satisfied {
            return Err(GenericError::ConditionUnsatisfied);
        }
        let network = self.cohort.network();
        let block_hash = self.cohort.block_hash().to_owned();
        let material = serde_json::json!({"domain":"minutus-generic-condition-proof-v1","network":network.descriptor().caip2_id(),"sources":self.source_commitments,"block":block_hash,"observations":self.observations});
        let proof_commitment = commitment(&material)?;
        // No collection or commitment work may renew the original finalisation
        // allowance. Check after all pure work, immediately before minting.
        diagnostics::simulation(Stage::ConditionSeal, control, || control.remaining()).map_err(
            |error| match error {
                evm_simulation::SimulationError::Cancelled
                | evm_simulation::SimulationError::Limit => GenericError::Expired,
                _ => GenericError::MissingState,
            },
        )?;
        // Debug output may itself block; it is not an allowance for late minting.
        control.remaining().map_err(|error| match error {
            evm_simulation::SimulationError::Cancelled | evm_simulation::SimulationError::Limit => {
                GenericError::Expired
            }
            _ => GenericError::MissingState,
        })?;
        if !self.cohort.is_fresh() {
            return Err(GenericError::Expired);
        }
        Ok(VerifiedGenericConditions {
            observed_at: self.cohort.selected_at,
            network,
            source_commitments: self.source_commitments,
            block_number: self.cohort.block_number(),
            block_hash,
            proof_commitment,
            observations: self.observations,
        })
    }
}

pub(super) fn latest(
    endpoint: &EvmRpcUrl,
    network: EvmNetwork,
    identities: &[&GenericContractIdentity],
    conditions: &[GenericCondition],
    cancelled: &AtomicBool,
) -> Result<PendingGenericConditions, GenericError> {
    // Keep the selector and proof-read controls in the same places as the
    // standalone public condition path. Finalisation uses the caller's control.
    let cohort = super::super::select_generic_live_cohort(endpoint, network, cancelled)?;
    if identities.is_empty() || identities.len() > 32 || conditions.len() > 16 {
        return Err(GenericError::Limit);
    }
    if !cohort.is_fresh() {
        return Err(GenericError::Expired);
    }
    let control = SimulationControl::start(cancelled);
    let reader = CandidateStateReader::at_cohort(endpoint, &cohort, &control)?;
    collect(&cohort, identities, conditions, &control, reader)
}

pub(super) fn collect(
    cohort: &GenericLiveCohort,
    identities: &[&GenericContractIdentity],
    conditions: &[GenericCondition],
    control: &SimulationControl<'_>,
    mut reader: CandidateStateReader<'_>,
) -> Result<PendingGenericConditions, GenericError> {
    if identities.is_empty() || identities.len() > 32 || conditions.len() > 16 {
        return Err(GenericError::Limit);
    }
    if !cohort.is_fresh() {
        return Err(GenericError::Expired);
    }
    let mut source_commitments = Vec::with_capacity(identities.len());
    for identity in identities {
        if identity.network() != cohort.network() {
            return Err(GenericError::SnapshotMismatch);
        }
        let queries = identity.state_queries();
        if queries.is_empty() || queries.len() > 32 {
            return Err(GenericError::Limit);
        }
        for query in queries {
            diagnostics::generic(Stage::SourceStateRead, control, || {
                reader.query(&query, control)
            })?;
        }
        source_commitments.push(identity.commitment().into());
    }
    let mut reads = BTreeMap::new();
    let mut observations = Vec::with_capacity(conditions.len());
    let mut satisfied = true;
    for condition in conditions {
        let observed = diagnostics::generic(Stage::ConditionRead, control, || {
            read(condition, identities, &mut reader, control, &mut reads)
        })?;
        satisfied &= diagnostics::generic(Stage::ConditionComparison, control, || {
            satisfies(condition, &observed.solidity_type, &observed.value)
        })?;
        observations.push(GenericConditionObservation {
            condition: condition.clone(),
            solidity_type: observed.solidity_type,
            value: observed.value,
        });
    }
    let state = diagnostics::generic(Stage::ConditionStateFinish, control, || {
        reader.finish_pending(cohort, control)
    })?;
    for identity in identities {
        diagnostics::generic(Stage::SourceIdentity, control, || {
            identity.verify_state(&state)
        })?;
    }
    if !cohort.is_fresh() {
        return Err(GenericError::Expired);
    }
    Ok(PendingGenericConditions {
        cohort: cohort.retained(),
        source_commitments,
        observations,
        satisfied,
    })
}

fn read(
    condition: &GenericCondition,
    identities: &[&GenericContractIdentity],
    reader: &mut CandidateStateReader<'_>,
    control: &SimulationControl<'_>,
    reads: &mut BTreeMap<(Address, U256), U256>,
) -> Result<layout::TypedStorageValue, GenericError> {
    let address = Address::from_str(&condition.target).map_err(|_| GenericError::InvalidAbi)?;
    if format!("{address:#x}") != condition.target {
        return Err(GenericError::InvalidAbi);
    }
    let artifact = condition_artifact(identities, address)?;
    layout::read_condition_path(
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
            if target != &condition.target {
                return Err(GenericError::SnapshotMismatch);
            }
            let slot = U256::from_str(slot).map_err(|_| GenericError::InvalidAbi)?;
            if let Some(value) = reads.get(&(address, slot)) {
                return Ok(*value);
            }
            if reads.len() >= 128 {
                return Err(GenericError::Limit);
            }
            let evm_simulation::StateValue::Storage { value } = reader.query(&query, control)?
            else {
                return Err(GenericError::MissingState);
            };
            let value = U256::from_str(&value).map_err(|_| GenericError::MissingState)?;
            reads.insert((address, slot), value);
            Ok(value)
        },
    )
}

#[cfg(test)]
mod tests;
