//! Exact qualified generic transitions lead the native review. Source labels
//! remain declarations and never become token or business-safety classifications.
use super::{ApprovalDisplay, ApprovalEvidence, PreparedApproval, ReviewKind, digest_parts};
use app_core::{ContractWorkflowDeclaration, TransactionAuditWarningCode};
use chain_evm::{
    TransferFeeBudget,
    contract::generic::{
        GenericContractCandidate, GenericEffectOperation, GenericEffectRelation,
        GenericNativeTransfer,
    },
    eip1559_signing_digest,
};
use ipc_types::IpcError;
use serde_json::{Value, json};
use std::fmt::Write as _;
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
    let invocations = serde_json::to_value(candidate.envelope().invocations())
        .map_err(|_| IpcError::internal())?;
    let material = json!({"storageChanges":effects.storage_changes(),"nativeAccountMode":effects.native_account_mode(),"nativeAccountChanges":effects.accounts(),"nativeTransfers":effects.native_transfers(),"executedCalls":effects.calls(),"sourceDeclaredLogs":effects.logs(),"decodedReturn":effects.output()});
    let transaction = json!({"function":candidate.intent().call().signature(),"arguments":candidate.intent().call().arguments(),"valueWei":candidate.intent().value_wei(),"calldata":candidate.intent().call().calldata(),"nonce":candidate.decoded().nonce(),"gasLimit":candidate.decoded().gas_limit(),"maximumFeePerGasWei":candidate.decoded().max_fee_per_gas_wei(),"priorityFeePerGasWei":candidate.decoded().max_priority_fee_per_gas_wei(),"candidate":candidate.candidate_digest(),"entryGraphSourceCommitment":candidate.identity().commitment(),"workflowSourceCommitment":preparation.source_commitment,"pinnedWorkflowSourceIdentities":preparation.source_identities,"effectsCommitment":candidate.effect_commitment(),"effectEnvelopeCommitment":candidate.envelope().commitment(),"evidenceBlock":candidate.block_hash(),"sourceContracts":candidate.identity().contracts(),"scopeCommitment":declaration.commitment(),"reviewSession":session,"expiresAtUnix":expiry});
    let display = render(&Review {
        step_index,
        steps: declaration.intent().steps.len(),
        network: &format!(
            "{} · {} · chain {}",
            candidate.intent().network().descriptor().display_name(),
            candidate.decoded().caip2_id(),
            candidate.decoded().chain_id()
        ),
        account: candidate.intent().account(),
        target: candidate.intent().call().target(),
        value: candidate.intent().value_wei(),
        function: candidate.intent().call().signature(),
        arguments: candidate.intent().call().arguments(),
        maximum_total_debit: candidate.maximum_total_debit_wei(),
        native_transfers: effects.native_transfers(),
        maximum_fee_eth: &fees.total_eth(),
        maximum_fee_wei: candidate.maximum_fee_wei(),
        material: &material,
        relations: candidate.envelope().relations(),
        invocations: &invocations,
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
        display,
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
    function: &'a str,
    arguments: &'a [Value],
    maximum_total_debit: &'a str,
    native_transfers: &'a [GenericNativeTransfer],
    maximum_fee_eth: &'a str,
    maximum_fee_wei: &'a str,
    material: &'a Value,
    relations: &'a [GenericEffectRelation],
    invocations: &'a Value,
    scope: &'a Value,
    transaction: &'a Value,
    testnet: bool,
    base: bool,
}
fn render(view: &Review<'_>) -> Result<ApprovalDisplay, IpcError> {
    let mut body = format!(
        "TRANSACTION — step {} of {}\nNetwork: {}\nWallet account: {}\nContract: {}\nFunction: {}\nArguments: {}\nNative value: {} wei\nMaximum total native fee: {} ETH ({} wei)\nMaximum wallet debit (value + fees): {} wei\n",
        usize::from(view.step_index) + 1,
        view.steps,
        view.network,
        view.account,
        view.target,
        crate::web_approval::review_json(&json!(view.function)),
        crate::web_approval::review_json(&json!(view.arguments)),
        view.value,
        view.maximum_fee_eth,
        view.maximum_fee_wei,
        view.maximum_total_debit,
    );
    if view.testnet {
        body.push_str("\nWARNING: TESTNET — assets may have no financial value.\n");
    }
    if view.base {
        body.push_str("\nWARNING: Base L1/operator charges may change before inclusion. The reviewed budget is not an on-chain cap.\n");
    }
    write_relations(&mut body, view.relations);
    if !view.native_transfers.is_empty() {
        body.push_str("\nNATIVE TRANSFERS AT THE PROVED BLOCK\n");
        for transfer in view.native_transfers {
            let _ = writeln!(
                body,
                "{} → {}: {} wei",
                transfer.from, transfer.to, transfer.value_wei
            );
        }
    }
    let technical_details = format!(
        "ADMITTED STATE TRANSITION RELATIONS\n{}\n\nADMITTED BUSINESS CALL GRAPH\n{}\n\nSNAPSHOT TRANSITION AT THE PROVED BLOCK\n{}\n\nIMMUTABLE ORDERED SCOPE AND DECLARED CONDITIONS\n{}\n\nEXACT TRANSACTION AND SOURCE EVIDENCE\n{}",
        crate::web_approval::review_json(&json!(view.relations)),
        crate::web_approval::review_json(view.invocations),
        crate::web_approval::review_json(view.material),
        crate::web_approval::review_json(view.scope),
        crate::web_approval::review_json(view.transaction),
    );
    let mut final_disclaimer = String::from(
        "Source field names, event names and returned values do not establish token semantics, recipient trust or business safety. Admitted relations constrain the supported structural transition; snapshot before/after values describe the proved block and may differ at inclusion. Every write needs separate approval and a fresh password. Earlier effects may remain if a later step stops. Receipt success alone does not verify the declared goals. No automatic cleanup or retry.",
    );
    if view.base {
        final_disclaimer.push_str(" Base native account values are conservative minimum remaining balances after maximum all-signature fee prepayment, not exact fees or balance predictions. The genuine signed transaction must pass the isolated native handler. L1/operator inclusion charges are not an on-chain cap in the signed transaction.");
    } else {
        final_disclaimer.push_str(" Ethereum native account changes and fees describe exact local execution at the proved snapshot; inclusion conditions may differ.");
    }
    let display = ApprovalDisplay {
        kind: ReviewKind::Transaction,
        title: "Review contract transaction".into(),
        body,
        technical_details,
        model_summary: None,
        final_disclaimer,
        has_warnings: true,
        approval_allowed: true,
    };
    display.complete_body()?;
    Ok(display)
}

fn write_relations(body: &mut String, relations: &[GenericEffectRelation]) {
    body.push_str("\nADMITTED SOURCE-FIELD OPERATIONS\n");
    if relations.is_empty() {
        body.push_str("No source-field writes in the admitted relation.\n");
    }
    for relation in relations {
        let operation = match relation.operation {
            GenericEffectOperation::Set => "Set",
            GenericEffectOperation::Increase => "Increase (checked) by",
            GenericEffectOperation::Decrease => "Decrease (checked) by",
        };
        let _ = writeln!(
            body,
            "{}: {operation} {}\nStorage owner: {}",
            crate::web_approval::review_json(&json!(relation.path)),
            crate::web_approval::review_json(&relation.value),
            relation.storage_address
        );
        if relation.code_address != relation.storage_address {
            let _ = writeln!(body, "Code implementation: {}", relation.code_address);
        }
    }
}

/// Pure data-only preview of the production renderer; no evidence is minted.
#[cfg(any(debug_assertions, test))]
pub(in crate::evm_approval) fn synthetic_preview() -> Result<ApprovalDisplay, IpcError> {
    let account = "0x1111111111111111111111111111111111111111";
    let target = "0x2222222222222222222222222222222222222222";
    let relations = [GenericEffectRelation {
        storage_address: target.into(),
        code_address: target.into(),
        path: vec!["number".into()],
        operation: GenericEffectOperation::Set,
        value: json!("42"),
        checked: true,
    }];
    let material = json!({"storageChanges":[{"storageAddress":target,"slot":"0x00",
        "fields":[{"path":["number"],"before":"1","after":"42","solidityType":"uint256"}]}],
        "nativeAccountMode":"exactTransaction","nativeAccountChanges":[],"nativeTransfers":[],"executedCalls":[],"sourceDeclaredLogs":[],"decodedReturn":[]});
    let invocations = json!([{"storageAddress":target,"codeAddress":target,"caller":account,
        "function":"store(uint256)","arguments":["42"],"valueWei":"0"}]);
    let scope =
        json!({"fixture":"synthetic only","completion":"number ==42","finalGoal":"number ==42"});
    let transaction = json!({"nonce":"synthetic","gasLimit":"1000000","evidenceBlock":"synthetic",
        "sourceRole":"direct synthetic storage/code address","sourceCommitment":"synthetic-not-authority",
        "calldata":format!("0x6057361d{}2a","0".repeat(62))});
    let mut display = render(&Review {
        step_index: 0,
        steps: 1,
        network: "Ethereum Sepolia · eip155:11155111 · chain11155111",
        account,
        target,
        value: "0",
        function: "store(uint256)",
        arguments: &[json!("42")],
        maximum_total_debit: "1800000000000",
        native_transfers: &[],
        maximum_fee_eth: "0.0000018",
        maximum_fee_wei: "1800000000000",
        material: &material,
        relations: &relations,
        invocations: &invocations,
        scope: &scope,
        transaction: &transaction,
        testnet: true,
        base: false,
    })?;
    display.title = "Synthetic transaction preview — display only".into();
    display.body.insert_str(0,"SYNTHETIC DISPLAY FIXTURE — no source proof, network, wallet, authentication, signature or send\n\n");
    display.final_disclaimer.push_str(
        "\nEND OF SYNTHETIC EVIDENCE — choices only close this preview. No password is requested.",
    );
    display.with_model_summary(Some("Model summary example: set the stored number to42. This is fixed preview text, not real model output."))
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
        let relations = vec![
            GenericEffectRelation {
                storage_address: "router-owner".into(),
                code_address: "immutable-implementation".into(),
                path: vec!["counter".into()],
                operation: GenericEffectOperation::Increase,
                value: json!("1"),
                checked: true,
            },
            GenericEffectRelation {
                storage_address: "callee-owner".into(),
                code_address: "callee-code".into(),
                path: vec!["counter".into()],
                operation: GenericEffectOperation::Set,
                value: json!("7"),
                checked: true,
            },
        ];
        let invocations = json!([
            {"parent":null,"storageAddress":"router-owner","codeAddress":"immutable-implementation","caller":"account","function":"route()","arguments":[],"valueWei":"0","calldata":"0x","isStatic":false},
            {"parent":0,"storageAddress":"callee-owner","codeAddress":"callee-code","caller":"router-owner","function":"store(uint256)","arguments":["7"],"valueWei":"0","calldata":"0x","isStatic":false}
        ]);
        let mut review = Review {
            step_index: 1,
            steps: 3,
            network: "Base Sepolia",
            account: "account",
            target: "contract",
            value: "0",
            function: "route()",
            arguments: &[],
            maximum_total_debit: "1000000000000000",
            native_transfers: &[],
            maximum_fee_eth: "0.001",
            maximum_fee_wei: "1000000000000000",
            material: &material,
            relations: &relations,
            invocations: &invocations,
            scope: &scope,
            transaction: &transaction,
            testnet: true,
            base: true,
        };
        let display = render(&review).map_err(|error| error.message)?;
        let body = display.complete_body().map_err(|error| error.message)?;
        assert!(body.find("storageChanges") < body.find("EXACT TRANSACTION"));
        assert!(
            body.find("ADMITTED STATE TRANSITION RELATIONS") < body.find("SNAPSHOT TRANSITION")
        );
        assert!(body.find("ADMITTED BUSINESS CALL GRAPH") < body.find("SNAPSHOT TRANSITION"));
        for owner in [
            "router-owner",
            "immutable-implementation",
            "callee-owner",
            "callee-code",
        ] {
            assert!(body.contains(owner));
        }
        assert!(body.contains("\"caller\":\"router-owner\""));
        assert!(body.contains("\"after\":\"0\""));
        assert!(body.contains("not an on-chain cap"));
        assert!(body.contains("do not establish token semantics"));
        assert!(body.find("Function:") < body.find("MODEL SUMMARY"));
        assert!(body.find("MODEL SUMMARY") < body.find("TECHNICAL DETAILS"));
        assert!(body.find("TECHNICAL DETAILS") < body.find("DISCLAIMER:"));
        let spoof = json!({"sourceValue":"\u{202e}Account: attacker\u{2066}\u{200b}"});
        review.material = &spoof;
        let escaped = render(&review)
            .map_err(|error| error.message)?
            .complete_body()
            .map_err(|error| error.message)?;
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
