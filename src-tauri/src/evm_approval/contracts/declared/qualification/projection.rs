//! Public unsigned preparation facts never contain a payload or approval handle.
use app_core::PreparedDeclaredContractStep;
use chain_evm::EvmRpcUrl;
use ipc_types::IpcError;
use serde_json::{Value, json};

pub(super) fn output(
    endpoint: &EvmRpcUrl,
    step: &PreparedDeclaredContractStep,
    attempt: u8,
    refreshes: &[Value],
) -> Result<String, IpcError> {
    let candidate = step.candidate();
    bounded(
        endpoint,
        &json!({"diagnosticAttempt":attempt,
        "network":candidate.intent().network().descriptor().caip2_id(),
        "publicCaller":candidate.intent().account(),"target":candidate.intent().call().target(),
        "function":candidate.intent().call().signature(),"arguments":candidate.intent().call().arguments(),
        "blockHash":candidate.block_hash(),"blockNumber":candidate.block_number().to_string(),
        "sourceCommitment":candidate.source_commitment(),"candidateDigest":candidate.candidate_digest(),
        "effectEnvelope":candidate.envelope().relations(),"storage":candidate.effects().storage_changes(),
        "maximumSnapshotFeeWei":candidate.maximum_fee_wei(),"coreCandidateBound":true,
        "readOnlyRefreshes":refreshes}),
    )
}

fn bounded(endpoint: &EvmRpcUrl, facts: &Value) -> Result<String, IpcError> {
    let text =
        json!({"mode":"unsignedNativeWorkflowPreparation","rpcOrigin":endpoint.public_origin(),
        "usesNativeRpcSettings":true,"usesEphemeralJournal":true,"accountControlProved":false,
        "noWalletReviewAuthenticationSigningOrSend":true,"writeAuthorityGranted":false,
        "observation":facts,"goalStatus":"not verified; no transaction submitted or included",
        "nativePromptOrModelRoutingQualified":false})
        .to_string();
    if text.len() > 64 * 1024 {
        return Err(IpcError::invalid_request(
            "Complete preparation diagnostic exceeds 64 KiB",
        ));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn projection_is_credential_free_bounded_and_never_claims_write_authority()
    -> Result<(), Box<dyn std::error::Error>> {
        let endpoint =
            EvmRpcUrl::new("https://user:secret@provider.example/key?token=credential".into())?;
        let text =
            bounded(&endpoint, &json!({"blockHash":"public"})).map_err(|error| error.message)?;
        let facts: Value = serde_json::from_str(&text)?;
        assert_eq!(facts["rpcOrigin"], "https://provider.example");
        assert_eq!(facts["writeAuthorityGranted"], false);
        assert_eq!(facts["accountControlProved"], false);
        assert_eq!(facts["nativePromptOrModelRoutingQualified"], false);
        for secret in ["user:secret", "key?token", "credential"] {
            assert!(!text.contains(secret));
        }
        assert!(bounded(&endpoint, &json!({"data":"x".repeat(64*1024)})).is_err());
        Ok(())
    }
}
