//! Independent canonical inclusion precedes candidate-bound completion proofs.
use super::{
    super::{context::Context, lifecycle::StepLifecycle},
    ObservationFailure,
};
use app_core::PreparedDeclaredContractStep;
use chain_evm::contract::{
    acquire_contract_transaction_proof,
    generic::{
        GenericCompletionFailure, GenericError, VerifiedGenericCompletion,
        VerifiedGenericInclusion, verify_generic_completion_staged,
    },
};
use ipc_types::IpcError;
use std::sync::Arc;

pub(super) enum ProofFailure {
    Inclusion(ObservationFailure),
    Completion {
        inclusion: VerifiedGenericInclusion,
        failure: ObservationFailure,
    },
    Worker,
}
impl ProofFailure {
    pub(super) fn record_stage(&self, progress: &mut StepLifecycle) -> Result<(), IpcError> {
        if let Self::Completion { inclusion, .. } = self {
            // Matching this branch requires the chain owner's private-field
            // seal. Neither an error code nor raw proof acquisition can enter.
            progress.record_transaction_hash(inclusion.transaction_hash())?;
            progress.inclusion_verified();
            progress.completion_started();
        }
        Ok(())
    }
}
impl From<GenericCompletionFailure> for ProofFailure {
    fn from(error: GenericCompletionFailure) -> Self {
        let diagnostic = error.diagnostic().copied();
        match error {
            GenericCompletionFailure::Inclusion(error) => Self::Inclusion(error.into()),
            GenericCompletionFailure::Completion { inclusion, error } => Self::Completion {
                inclusion,
                failure: diagnosed_failure(error, diagnostic),
            },
        }
    }
}

fn diagnosed_failure(
    error: GenericError,
    diagnostic: Option<chain_evm::contract::generic::CompletionDiagnostic>,
) -> ObservationFailure {
    let mut failure = ObservationFailure::from(error);
    if let (ObservationFailure::Unknown(error), Some(diagnostic)) = (&mut failure, diagnostic) {
        use std::fmt::Write as _;
        let _ = write!(error.message, "; {diagnostic}");
    }
    failure
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
        verify_generic_completion_staged(
            &endpoint,
            step.candidate(),
            &identities,
            &hash,
            &inclusion,
            &conditions,
            &cancelled,
        )
        .map_err(ProofFailure::from)
    })
    .await
    .map_err(|_| ProofFailure::Worker)?
}
