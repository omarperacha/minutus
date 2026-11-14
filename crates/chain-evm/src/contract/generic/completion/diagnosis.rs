//! Read-only reproduction of completion's original control and final anchors.
//! Public inclusion here cannot stand in for the original approved candidate.
use super::{
    GenericCondition, GenericContractIdentity, GenericError, Phase, Recorder, Stage,
    VerifiedGenericConditions, diagnostics, final_control, finish_conditions,
};
use crate::{EvmNetwork, EvmRpcUrl, contract::DeploymentProof, local_simulation};
use evm_simulation::SimulationControl;
use std::sync::atomic::AtomicBool;

/// Diagnoses the production condition/final-anchor sequence for public inclusion.
/// It shares the exact condition phase and original outer control with production
/// completion, but never constructs a candidate or full completion certificate.
/// Returned conditions are current observations only; they do not attribute
/// them to this transaction, assert original approved-body binding, or sign.
/// This developer-only API requires explicit existing outbound consent.
///
/// # Errors
/// Rejects malformed/failed public inclusion, unavailable proof/source/state,
/// false conditions, cancellation, expired controls and noncanonical anchors.
pub fn diagnose_generic_receipt_conditions(
    endpoint: &EvmRpcUrl,
    network: EvmNetwork,
    inclusion: &DeploymentProof,
    identities: &[&GenericContractIdentity],
    conditions: &[GenericCondition],
    cancelled: &AtomicBool,
) -> Result<VerifiedGenericConditions, GenericError> {
    let control = SimulationControl::start(cancelled);
    let header = diagnostics::simulation(Stage::ReceiptHeader, &control, || {
        local_simulation::canonical_header(endpoint, network, &inclusion.block_hash, &control)
    })
    .map_err(|_| GenericError::SnapshotMismatch)?;
    diagnostics::generic(Stage::ExactInclusion, &control, || {
        crate::contract::qualification::verify_public_successful_inclusion(
            network, &header, inclusion,
        )
        .map_err(|_| GenericError::SnapshotMismatch)
    })?;
    let mut diagnostic = None;
    let mut recorder = Recorder::new(&control, &mut diagnostic);
    let observed = finish_conditions(
        endpoint,
        network,
        identities,
        (&inclusion.block_hash, header.number),
        conditions,
        cancelled,
        &mut recorder,
    )?;
    recorder.generic(Phase::FinalCurrentAnchor, || {
        final_control(&control, &observed)
    })?;
    Ok(observed)
}
