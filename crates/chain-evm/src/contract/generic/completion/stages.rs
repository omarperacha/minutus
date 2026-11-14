//! Only the exact inclusion verifier can produce the post-inclusion stage seal.
use super::{CompletionDiagnostic, GenericError, commitment};
use crate::{
    EvmNetwork,
    contract::{DeploymentProof, qualification::verify_transaction_inclusion},
};
use alloy_consensus::Header;

/// Exact successful candidate-body/signer and transaction/receipt MPT inclusion
/// at a host-checked canonical header. This historical stage does not establish
/// source conditions, later canonicality, task completion or signing authority.
/// It has no raw, clone, serialization or deserialization constructor.
pub struct VerifiedGenericInclusion {
    pub(super) transaction_hash: String,
    pub(super) receipt_number: u64,
    pub(super) receipt_hash: String,
    pub(super) inclusion_commitment: String,
    diagnostic: Option<Box<CompletionDiagnostic>>,
}
impl VerifiedGenericInclusion {
    /// Exact signed transaction whose inclusion passed verification.
    #[must_use]
    pub fn transaction_hash(&self) -> &str {
        &self.transaction_hash
    }
}

/// Failed generic verification with only stages actually established by the host.
pub enum GenericCompletionFailure {
    /// No usable exact canonical candidate inclusion seal was established.
    Inclusion(GenericError),
    /// Exact inclusion was checked before source/state/final-anchor verification
    /// failed. The seal does not assert that the old anchor remains canonical.
    Completion {
        /// Independently checked historical inclusion stage.
        inclusion: VerifiedGenericInclusion,
        /// Original unchanged completion verification error.
        error: GenericError,
    },
}
impl GenericCompletionFailure {
    /// Closed post-inclusion failure facts, with no change to the legacy error.
    #[must_use]
    pub fn diagnostic(&self) -> Option<&CompletionDiagnostic> {
        match self {
            Self::Inclusion(_) => None,
            Self::Completion { inclusion, .. } => inclusion.diagnostic.as_deref(),
        }
    }
    /// Returns the original legacy error without granting stage evidence.
    #[must_use]
    pub fn into_error(self) -> GenericError {
        match self {
            Self::Inclusion(error) | Self::Completion { error, .. } => error,
        }
    }
}

#[cfg(test)]
pub(super) fn complete<T>(
    inclusion: VerifiedGenericInclusion,
    operation: impl FnOnce(&VerifiedGenericInclusion) -> Result<T, GenericError>,
) -> Result<T, GenericCompletionFailure> {
    complete_diagnosed(inclusion, |checked, _| operation(checked))
}

pub(super) fn complete_diagnosed<T>(
    mut inclusion: VerifiedGenericInclusion,
    operation: impl FnOnce(
        &VerifiedGenericInclusion,
        &mut Option<CompletionDiagnostic>,
    ) -> Result<T, GenericError>,
) -> Result<T, GenericCompletionFailure> {
    let mut diagnostic = None;
    operation(&inclusion, &mut diagnostic).map_err(|error| {
        inclusion.diagnostic = diagnostic.map(Box::new);
        GenericCompletionFailure::Completion { inclusion, error }
    })
}

pub(super) fn verify(
    network: EvmNetwork,
    header: &Header,
    payload: &[u8],
    account: &str,
    hash: &str,
    proof: &DeploymentProof,
    candidate_height: u64,
) -> Result<VerifiedGenericInclusion, GenericError> {
    verify_transaction_inclusion(network, header, payload, account, hash, proof)
        .map_err(|_| GenericError::SnapshotMismatch)?;
    if header.number < candidate_height {
        return Err(GenericError::SnapshotMismatch);
    }
    Ok(VerifiedGenericInclusion {
        transaction_hash: hash.into(),
        receipt_number: header.number,
        receipt_hash: proof.block_hash.clone(),
        inclusion_commitment: commitment(proof)?,
        diagnostic: None,
    })
}

#[cfg(test)]
mod tests;
