//! The real qualification callbacks run without review, authentication or signing.
use super::super::context::Context;
use app_core::PreparedDeclaredContractStep;
use ipc_types::IpcError;
use serde_json::{Value, json};

pub(super) async fn twice(
    context: &Context,
    prepared: &PreparedDeclaredContractStep,
) -> Result<Vec<Value>, IpcError> {
    let mut results = Vec::with_capacity(2);
    for stage in ["PostAuthenticationEquivalent", "PreSubmissionEquivalent"] {
        eprintln!("Contract preparation stage: {stage}");
        let (fresh, proof) = context.refresh(prepared, &[]).await?;
        let facts = json!({"stage":stage,"actualApprovalAuthenticationOrSignature":false,
            "blockHash":fresh.block_hash(),"blockNumber":fresh.block_number().to_string(),
            "freshCandidateDigest":fresh.fresh().candidate_digest(),
            "prerequisiteBlockHash":proof.conditions().block_hash(),
            "prerequisiteBlockNumber":proof.conditions().block_number().to_string(),
            "approvedCandidateDigest":proof.candidate_digest()});
        prepared.revalidate_preparation(proof).map_err(|_| {
            IpcError::invalid_request("Ephemeral refreshed prerequisite binding failed")
        })?;
        results.push(facts);
    }
    Ok(results)
}
