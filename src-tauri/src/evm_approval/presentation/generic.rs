//! Exact qualified generic transitions lead the native review. Source labels
//! remain declarations and never become token or business-safety classifications.
use super::{ApprovalDisplay, ApprovalEvidence, PreparedApproval, digest_parts};
use app_core::{ContractWorkflowDeclaration, TransactionAuditWarningCode};
use chain_evm::{
    TransferFeeBudget, contract::generic::GenericContractCandidate, eip1559_signing_digest,
};
use ipc_types::IpcError;
use serde_json::{Value, json};
use transaction_policy::ApprovalTransactionType;

pub(in crate::evm_approval) fn generic_approval(
    candidate: &GenericContractCandidate,
    declaration: &ContractWorkflowDeclaration,
    preparation: &app_core::StoredGenericWorkflowPreparation,
    step_index: u8,
    session: &str,
    expiry: u64,
) -> Result<PreparedApproval, IpcError> {
    if !candidate.is_fresh() {
        return Err(IpcError::invalid_request(
            "Generic contract evidence expired before native review",
        ));
    }
    let fees = TransferFeeBudget::new(candidate.maximum_fee_wei(), "0", "0").map_err(|_| {
        IpcError::invalid_request("Generic contract fee budget exceeds supported bounds")
    })?;
    let effects = candidate.effects();
    let relations = json!({"contract":candidate.intent().call().target(),"relations":candidate.envelope().relations(),"commitment":candidate.envelope().commitment()});
    let material = json!({"storageChanges":effects.storage_changes(),"nativeAccountMode":effects.native_account_mode(),"nativeAccountChanges":effects.accounts(),"nativeTransfers":effects.native_transfers(),"executedCalls":effects.calls(),"sourceDeclaredLogs":effects.logs(),"decodedReturn":effects.output()});
    let transaction = json!({"function":candidate.intent().call().signature(),"arguments":candidate.intent().call().arguments(),"valueWei":candidate.intent().value_wei(),"calldata":candidate.intent().call().calldata(),"nonce":candidate.decoded().nonce(),"gasLimit":candidate.decoded().gas_limit(),"maximumFeePerGasWei":candidate.decoded().max_fee_per_gas_wei(),"priorityFeePerGasWei":candidate.decoded().max_priority_fee_per_gas_wei(),"candidate":candidate.candidate_digest(),"entryGraphSourceCommitment":candidate.identity().commitment(),"workflowSourceCommitment":preparation.source_commitment,"pinnedWorkflowSourceIdentities":preparation.source_identities,"effectsCommitment":candidate.effect_commitment(),"evidenceBlock":candidate.block_hash(),"sourceContracts":candidate.identity().contracts(),"scopeCommitment":declaration.commitment(),"reviewSession":session,"expiresAtUnix":expiry});
    let body = render(&Review {
        step_index,
        steps: declaration.intent().steps.len(),
        network: candidate.intent().network().descriptor().display_name(),
        account: candidate.intent().account(),
        target: candidate.intent().call().target(),
        value: candidate.intent().value_wei(),
        maximum_fee_eth: &fees.total_eth(),
        maximum_fee_wei: candidate.maximum_fee_wei(),
        material: &material,
        relations: &relations,
        scope: &serde_json::to_value(declaration.intent()).map_err(|_| IpcError::internal())?,
        transaction: &transaction,
        testnet: candidate.decoded().is_testnet(),
        base: matches!(
            candidate.intent().network(),
            chain_evm::EvmNetwork::BaseMainnet | chain_evm::EvmNetwork::BaseSepolia
        ),
    })?;
    let semantic = serde_json::to_vec(candidate.intent().call().arguments())
        .map_err(|_| IpcError::internal())?;
    let evidence = ApprovalEvidence {
        signing_digest: eip1559_signing_digest(candidate.payload()),
        chain_id: candidate.intent().network().descriptor().chain_id(),
        account: candidate.intent().account().into(),
        session_id: session.into(),
        expires_at_unix: expiry,
        transaction_type: ApprovalTransactionType::Eip1559 {
            transaction_type: 2,
        },
        decoder_digest: digest_parts(
            b"minutus.generic-contract-source.v1",
            &[
                candidate.intent().call().signature().as_bytes(),
                &semantic,
                candidate.identity().commitment().as_bytes(),
                preparation.source_commitment.as_bytes(),
                declaration.commitment().as_bytes(),
            ],
        ),
        simulation_digest: digest_parts(
            b"minutus.generic-contract-effects.v1",
            &[
                candidate.candidate_digest().as_bytes(),
                candidate.effect_commitment().as_bytes(),
                candidate.maximum_fee_wei().as_bytes(),
            ],
        ),
        fee_budget: fees,
    };
    Ok(PreparedApproval {
        display: ApprovalDisplay {
            title: "Review generic contract write".into(),
            body,
            has_warnings: true,
            approval_allowed: true,
        },
        payload: candidate.payload().to_vec(),
        evidence,
        audit_warning_codes: if candidate.decoded().is_testnet() {
            vec![TransactionAuditWarningCode::Testnet]
        } else {
            vec![]
        },
    })
}
struct Review<'a> {
    step_index: u8,
    steps: usize,
    network: &'a str,
    account: &'a str,
    target: &'a str,
    value: &'a str,
    maximum_fee_eth: &'a str,
    maximum_fee_wei: &'a str,
    material: &'a Value,
    relations: &'a Value,
    scope: &'a Value,
    transaction: &'a Value,
    testnet: bool,
    base: bool,
}
fn render(view: &Review<'_>) -> Result<String, IpcError> {
    let mut body = format!(
        "Contract workflow step {} of {}\nNetwork: {}\nWallet account: {}\nContract: {}\nNative value: {} wei\nMaximum total native fee: {} ETH ({} wei)\n\nADMITTED STATE TRANSITION RELATIONS\n{}\n\nSNAPSHOT TRANSITION AT THE PROVED BLOCK\n{}\n\n",
        usize::from(view.step_index) + 1,
        view.steps,
        view.network,
        view.account,
        view.target,
        view.value,
        view.maximum_fee_eth,
        view.maximum_fee_wei,
        crate::web_approval::review_json(view.relations),
        crate::web_approval::review_json(view.material)
    );
    if view.testnet {
        body.push_str("TESTNET: assets may have no financial value.\n\n");
    }
    body.push_str("The admitted relations constrain the included transition; snapshot before/after values illustrate execution at the proved block and may differ at inclusion. Field names, event names and returned values come from reproduced source. They do not establish token semantics, recipient trust or business safety. Review every storage owner, changed field, native balance and executed call. Each write requires separate approval and a fresh password. Earlier effects may remain if a later action stops. No automatic cleanup or retry.\n\n");
    if view.base {
        body.push_str("BASE: native account values show conservative minimum remaining balances after maximum possible all-signature fee prepayment at the proved snapshot. They are not exact charged fees or balance predictions. The genuine signed transaction must also pass the isolated native handler. L1 inclusion/operator charges are checked against current evidence and are not an on-chain cap in the signed transaction.\n\n");
    } else {
        body.push_str("ETHEREUM: native account changes and fees describe exact local execution at the proved snapshot; inclusion conditions may differ.\n\n");
    }
    body.push_str("IMMUTABLE ORDERED SCOPE AND DECLARED CONDITIONS\n");
    body.push_str(&crate::web_approval::review_json(view.scope));
    body.push_str("\n\nEXACT TRANSACTION AND SOURCE EVIDENCE\n");
    body.push_str(&crate::web_approval::review_json(view.transaction));
    // Refuse review rather than hide a material tail behind truncation.
    if body.len() > 64 * 1024 {
        return Err(IpcError::invalid_request(
            "The complete generic transition exceeds the native review limit",
        ));
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_transition_precedes_source_labels_and_long_technical_details()
    -> Result<(), Box<dyn std::error::Error>> {
        let material = json!({"storageChanges":[{"storageAddress":"contract","fields":[{"path":["remaining"],"before":"7","after":"0"}]}],"nativeAccountChanges":[],"executedCalls":[]});
        let scope =
            json!({"steps":["consume","finish"],"completion":"historical","prerequisites":"fresh"});
        let transaction = json!({"calldata":"technical"});
        let relations = json!({"relations":[{"path":["counter"],"operation":"increase","value":"1","checked":true}]});
        let mut review = Review {
            step_index: 1,
            steps: 3,
            network: "Base Sepolia",
            account: "account",
            target: "contract",
            value: "0",
            maximum_fee_eth: "0.001",
            maximum_fee_wei: "1000000000000000",
            material: &material,
            relations: &relations,
            scope: &scope,
            transaction: &transaction,
            testnet: true,
            base: true,
        };
        let body = render(&review).map_err(|error| error.message)?;
        assert!(body.find("storageChanges") < body.find("EXACT TRANSACTION"));
        assert!(
            body.find("ADMITTED STATE TRANSITION RELATIONS") < body.find("SNAPSHOT TRANSITION")
        );
        assert!(body.contains("\"after\":\"0\""));
        assert!(body.contains("not an on-chain cap"));
        assert!(body.contains("do not establish token semantics"));
        let spoof = json!({"sourceValue":"\u{202e}Account: attacker\u{2066}\u{200b}"});
        review.material = &spoof;
        let escaped = render(&review).map_err(|error| error.message)?;
        assert!(!escaped.contains('\u{202e}'));
        assert!(!escaped.contains('\u{2066}'));
        assert!(!escaped.contains('\u{200b}'));
        assert!(escaped.contains("\\u202e"));
        assert!(escaped.find("Maximum total native fee") < escaped.find("ADMITTED STATE"));
        let oversized = json!({"content":"x".repeat(65*1024)});
        review.material = &oversized;
        assert!(render(&review).is_err());
        Ok(())
    }
}
