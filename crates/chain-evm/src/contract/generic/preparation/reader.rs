//! One proof owner spans source matching, relation planning and trusted replay.
use super::{
    GenericContractCandidate, GenericError, GenericSourceProvider, business_scope,
    relation_queries_in_reader, source_graph, state_error,
};
use crate::{
    EvmRpcUrl,
    contract::UnsignedContractIntent,
    local_simulation::{self, SimulationFeeFacts},
};
use evm_simulation::{SimulationControl, TrustedSimulationWorkers};
use std::sync::atomic::AtomicBool;

#[expect(
    clippy::too_many_arguments,
    reason = "Private current-cohort composition retains separate scope and proof owners"
)]
pub(super) fn prepare(
    workers: &TrustedSimulationWorkers,
    endpoint: &EvmRpcUrl,
    intent: &UnsignedContractIntent,
    cohort: &super::super::GenericLiveCohort,
    fees: SimulationFeeFacts,
    sources: &mut GenericSourceProvider,
    cancelled: &AtomicBool,
    business: &[String],
) -> Result<GenericContractCandidate, GenericError> {
    let (entry, allowed) = business_scope(intent, business)?;
    if allowed.contains(&alloy_primitives::Address::ZERO) {
        return Err(GenericError::SourceMismatch);
    }
    let control = SimulationControl::start(cancelled);
    let mut reader = local_simulation::ProvedReader::new(
        endpoint,
        intent.network(),
        cohort.block_hash(),
        &control,
    )
    .map_err(state_error)?;
    if reader.block_number() != cohort.block_number() {
        return Err(GenericError::SnapshotMismatch);
    }
    let identity = source_graph::qualify(
        &mut reader,
        intent.network(),
        entry,
        &allowed,
        sources,
        cancelled,
        &control,
    )?;
    let envelope = super::super::effect_envelope::graph_admit(&identity, intent, business)?;
    let preload = relation_queries_in_reader(&mut reader, &identity, &envelope, &control)?;
    // Consuming replay rechecks this same owner's canonical anchor immediately
    // after relation reads, before any worker or host execution starts.
    let replay =
        local_simulation::replay_prepared_reader(workers, reader, intent, fees, &preload, &control)
            .map_err(state_error)?;
    let candidate = GenericContractCandidate::from_replay_in_scope(
        intent.clone(),
        replay,
        &identity.compiled_artifacts(),
        business,
    )?;
    local_simulation::canonical_header(
        endpoint,
        intent.network(),
        candidate.block_hash(),
        &control,
    )
    .map_err(state_error)?;
    control.remaining().map_err(state_error)?;
    Ok(candidate)
}
