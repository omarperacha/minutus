//! Independent canonical inclusion precedes candidate-bound completion proofs.
use super::{super::context::Context, ObservationFailure};
use app_core::PreparedDeclaredContractStep;
use chain_evm::contract::{
    acquire_contract_transaction_proof,
    generic::{GenericError, VerifiedGenericCompletion, verify_generic_completion},
};
use std::sync::Arc;

pub(super) enum ProofFailure {
    Inclusion(ObservationFailure),
    Completion(ObservationFailure),
    Worker,
}

pub(super) async fn prove(
    context: &Context,
    step: Arc<PreparedDeclaredContractStep>,
    hash: String,
) -> Result<VerifiedGenericCompletion, ProofFailure> {
    let network = context.network;
    let endpoint = Arc::clone(&context.endpoint);
    let cancelled = Arc::clone(&context.cancelled);
    let identities = context.identity_views();
    let prepared = context
        .candidate_views()
        .map_err(|error| ProofFailure::Inclusion(ObservationFailure::Unknown(error)))?;
    let conditions = step.completion_conditions().map_err(|error| {
        ProofFailure::Inclusion(ObservationFailure::Unknown(
            crate::error_mapping::app_core_error(error.into()),
        ))
    })?;
    tokio::task::spawn_blocking(move || {
        let inclusion =
            acquire_contract_transaction_proof(network, &endpoint, &hash, &cancelled)
                .map_err(|_| ProofFailure::Inclusion(GenericError::SnapshotMismatch.into()))?;
        let mut identities = identities.iter().map(Arc::as_ref).collect::<Vec<_>>();
        for previous in &prepared {
            if !identities
                .iter()
                .any(|identity| identity.commitment() == previous.identity().commitment())
            {
                identities.push(previous.identity());
            }
        }
        if !identities
            .iter()
            .any(|identity| identity.commitment() == step.candidate().identity().commitment())
        {
            identities.push(step.candidate().identity());
        }
        verify_generic_completion(
            &endpoint,
            step.candidate(),
            &identities,
            &hash,
            &inclusion,
            &conditions,
            &cancelled,
        )
        .map_err(|error| ProofFailure::Completion(error.into()))
    })
    .await
    .map_err(|_| ProofFailure::Worker)?
}
