//! Source-only workflow preflight, before any step can request approval or signing.
use super::{
    DeploymentProof, QualificationError, QualifiedContractIdentity, action::parse_address,
    deployment::VerifiedDeployment,
};
use crate::{EvmNetwork, EvmRpcUrl, local_simulation};
use evm_simulation::{SimulationControl, StateQuery};
use std::sync::atomic::AtomicBool;

/// Proves a workflow target's current exact source and legitimate creation.
/// This certificate contains no action, effects, fee, candidate or spend authority.
/// Native outbound scope permission must precede this public RPC operation.
///
/// # Errors
/// Rejects unsupported code/proxy/creation, missing proofs, future deployment,
/// changed latest tip, reorg, unavailable state, cancellation and resource limits.
pub fn qualify_contract_workflow_source(
    network: EvmNetwork,
    endpoint: &EvmRpcUrl,
    target: &str,
    deployment: &DeploymentProof,
    cancelled: &AtomicBool,
) -> Result<QualifiedContractIdentity, QualificationError> {
    let target = parse_address(target)?;
    let control = SimulationControl::start(cancelled);
    let creation =
        local_simulation::canonical_header(endpoint, network, &deployment.block_hash, &control)?;
    let verified =
        VerifiedDeployment::verify_against_header(&creation, network, target, deployment)?;
    let latest = local_simulation::latest_header(endpoint, network, &control)?;
    let block_hash = format!("{:#x}", latest.hash_slow());
    let queries = [StateQuery::Account {
        address: format!("{target:#x}"),
    }];
    let (number, state) =
        local_simulation::read_proved(endpoint, network, &block_hash, &queries, &control)?;
    let identity = QualifiedContractIdentity::from_snapshot(
        network,
        target,
        &block_hash,
        number,
        &state,
        verified,
    )?;
    local_simulation::canonical_header(endpoint, network, &deployment.block_hash, &control)?;
    local_simulation::canonical_header(endpoint, network, &block_hash, &control)?;
    if local_simulation::latest_header(endpoint, network, &control)?.hash_slow()
        != latest.hash_slow()
    {
        return Err(QualificationError::SnapshotMismatch);
    }
    control.remaining()?;
    Ok(identity)
}

#[cfg(test)]
mod tests;
