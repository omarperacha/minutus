//! Source-qualified material facts lead the native review; technical bindings
//! remain selectable below them. Presentation data can never mint approval.
use super::{ApprovalDisplay, ApprovalEvidence, PreparedApproval, ReviewKind, digest_parts};
use chain_evm::{
    TransferFeeBudget,
    contract::{AllowanceEffect, ContractAction, QualifiedContractCandidate, TokenBalanceEffect},
    eip1559_signing_digest,
};
use ipc_types::IpcError;
use std::fmt::Write as _;
use transaction_policy::ApprovalTransactionType;

pub(in crate::evm_approval) fn qualified_approval(
    candidate: &QualifiedContractCandidate,
    scope: &app_core::ContractWorkflowScope,
    step_index: u8,
    session_id: &str,
    expires_at_unix: u64,
) -> Result<PreparedApproval, IpcError> {
    let semantic = serde_json::to_vec(candidate.action()).map_err(|_| IpcError::internal())?;
    let fees = TransferFeeBudget::new(candidate.maximum_fee_wei(), "0", "0")
        .map_err(|_| IpcError::invalid_request("Contract fee budget exceeds supported bounds"))?;
    let scope_text = crate::web_approval::review_json(
        &serde_json::to_value(scope.intent()).map_err(|_| IpcError::internal())?,
    );
    let details = technical_details(candidate, scope, session_id, expires_at_unix);
    let fee_eth = fees.total_eth();
    let network = format!(
        "{} · {} · chain {}",
        candidate.intent().network().descriptor().display_name(),
        candidate.decoded().caip2_id(),
        candidate.decoded().chain_id()
    );
    let display = render(&ReviewBody {
        step_index,
        steps: scope.intent().steps.len(),
        network: &network,
        account: candidate.intent().account(),
        target: candidate.identity().target(),
        action: candidate.action(),
        function: candidate.intent().call().signature(),
        arguments: candidate.intent().call().arguments(),
        allowance: candidate.allowance(),
        balances: candidate.balances(),
        maximum_fee_eth: &fee_eth,
        maximum_fee_wei: candidate.maximum_fee_wei(),
        scope: &scope_text,
        details: &details,
        base: matches!(candidate, QualifiedContractCandidate::Base(_)),
        testnet: candidate.decoded().is_testnet(),
    });
    let mut audit_warning_codes = Vec::new();
    if candidate.decoded().is_testnet() {
        audit_warning_codes.push(app_core::TransactionAuditWarningCode::Testnet);
    }
    if candidate
        .allowance()
        .is_some_and(|allowance| allowance.unlimited)
    {
        audit_warning_codes.push(app_core::TransactionAuditWarningCode::UnlimitedApproval);
    }
    Ok(PreparedApproval {
        display,
        payload: candidate.payload().to_vec(),
        evidence: qualified_evidence(
            candidate,
            scope,
            session_id,
            expires_at_unix,
            fees,
            &semantic,
        ),
        audit_warning_codes,
    })
}

// Data-only private view: only qualified_approval constructs ApprovalEvidence.
struct ReviewBody<'a> {
    step_index: u8,
    steps: usize,
    network: &'a str,
    account: &'a str,
    target: &'a str,
    action: &'a ContractAction,
    function: &'a str,
    arguments: &'a [serde_json::Value],
    allowance: Option<&'a AllowanceEffect>,
    balances: &'a [TokenBalanceEffect],
    maximum_fee_eth: &'a str,
    maximum_fee_wei: &'a str,
    scope: &'a str,
    details: &'a str,
    base: bool,
    testnet: bool,
}
fn render(view: &ReviewBody<'_>) -> ApprovalDisplay {
    let mut body = format!(
        "Contract workflow step {} of {}\nNetwork: {}\nWallet account: {}\nToken contract: {}\n\n",
        usize::from(view.step_index) + 1,
        view.steps,
        view.network,
        view.account,
        view.target
    );
    if view.testnet {
        body.push_str("TESTNET: assets on this network may have no financial value.\n\n");
    }
    match view.action {
        ContractAction::Approve {
            spender, amount, ..
        } => {
            let _ = writeln!(
                body,
                "ACTION: Set token allowance\nSpender: {spender}\nNew allowance: {amount} raw units"
            );
        }
        ContractAction::Transfer { from, to, amount } => {
            let _ = writeln!(
                body,
                "ACTION: Transfer tokens\nFrom: {from}\nRecipient: {to}\nAmount: {amount} raw units"
            );
        }
        ContractAction::TransferFrom {
            spender,
            from,
            to,
            amount,
        } => {
            let _ = writeln!(
                body,
                "ACTION: Spend token allowance and transfer\nSpender: {spender}\nFrom: {from}\nRecipient: {to}\nAmount: {amount} raw units"
            );
        }
    }
    let _ = writeln!(
        body,
        "Function: {}\nArguments: {}",
        crate::web_approval::review_json(&serde_json::json!(view.function)),
        crate::web_approval::review_json(&serde_json::json!(view.arguments))
    );
    if let Some(allowance) = view.allowance {
        let _ = writeln!(
            body,
            "\nALLOWANCE\nOwner: {}\nSpender: {}\nBefore: {} raw units\nAfter: {} raw units\nUnlimited authority: {}",
            allowance.owner,
            allowance.spender,
            allowance.before,
            allowance.after,
            allowance.unlimited
        );
        if allowance.unlimited {
            body.push_str("WARNING: remaining allowance is UNLIMITED (maximum uint256).\n");
        }
        if allowance.before != allowance.after && allowance.after != "0" {
            body.push_str("The displayed spender may transfer tokens up to this allowance. Source qualification does not establish that the spender is trustworthy.\n");
        }
    }
    for balance in view.balances {
        let _ = writeln!(
            body,
            "\nTOKEN BALANCE\nHolder: {}\nBefore: {} raw units\nAfter: {} raw units",
            balance.account, balance.before, balance.after
        );
    }
    let _ = writeln!(
        body,
        "\nNATIVE VALUE: 0 wei\nMAXIMUM TOTAL NATIVE FEE\n{} ETH ({} wei)\nMaximum wallet debit (fees only): {} wei",
        view.maximum_fee_eth, view.maximum_fee_wei, view.maximum_fee_wei
    );
    if view.base {
        body.push_str("\nWARNING: Base L1/operator charges may change before inclusion. This budget is not an on-chain cap.\n");
    }
    let technical_details = format!(
        "FULL IMMUTABLE ORDERED SCOPE\n{}\n\nEXACT TRANSACTION AND SOURCE EVIDENCE\n{}",
        view.scope, view.details
    );
    let mut final_disclaimer = String::from(
        "Source qualification does not establish that the spender or recipient is trustworthy. The allowance and balance changes describe the proved snapshot; inclusion conditions may differ. Each write requires separate approval and a fresh password. An allowance may remain if a later step stops. Receipt success alone does not complete the declared task. No automatic cleanup or retry.",
    );
    if view.base {
        final_disclaimer.push_str(" Base material effects are qualified before signing; the genuine signed transaction must pass the isolated native handler before submission. L1/operator inclusion charges are checked against current evidence and are not an on-chain cap in the signed transaction.");
    }
    ApprovalDisplay {
        kind: ReviewKind::Transaction,
        title: "Review token transaction".into(),
        body,
        technical_details,
        model_summary: None,
        final_disclaimer,
        has_warnings: true,
        approval_allowed: true,
    }
}

fn technical_details(
    candidate: &QualifiedContractCandidate,
    scope: &app_core::ContractWorkflowScope,
    session_id: &str,
    expiry: u64,
) -> String {
    let intent = candidate.intent();
    let decoded = candidate.decoded();
    let identity = candidate.identity();
    format!(
        "Function: {}\nArguments (raw units): {}\nNonce: {}\nGas limit: {}\nMax fee per gas: {} wei\nPriority fee per gas: {} wei\nReviewed source profile: {}\nSource revision: {}\nRuntime hash: {}\nDeployment transaction: {}\nEvidence block: {}\nExact calldata: {}\nCandidate: {}\nScope commitment: {}\nReview session: {}\nExpires at UNIX time: {}\n",
        intent.call().signature(),
        crate::web_approval::review_json(&serde_json::Value::Array(
            intent.call().arguments().to_vec()
        )),
        decoded.nonce(),
        decoded.gas_limit(),
        decoded.max_fee_per_gas_wei(),
        decoded.max_priority_fee_per_gas_wei(),
        identity.profile_id(),
        identity.profile_revision(),
        identity.runtime_hash(),
        identity.deployment_transaction_hash(),
        identity.block_hash(),
        intent.call().calldata(),
        candidate.candidate_digest(),
        scope.commitment(),
        session_id,
        expiry
    )
}

/// Fixed display-only debug fixture uses exactly the production data renderer.
/// No qualified candidate, `ApprovalEvidence` or signing handle is constructed.
#[cfg(test)]
pub(in crate::evm_approval) fn synthetic_preview() -> ApprovalDisplay {
    let account = "0x1111111111111111111111111111111111111111";
    let action = ContractAction::Approve {
        owner: account.into(),
        spender: account.into(),
        amount: "100".into(),
    };
    let allowance = AllowanceEffect {
        owner: account.into(),
        spender: account.into(),
        before: "0".into(),
        after: "100".into(),
        unlimited: false,
    };
    let scope = "1. Approve own account for100 raw units.\n2. transferFrom own account to0x3333333333333333333333333333333333333333,100 raw units.\nEvery write still needs separate native approval and fresh authentication.";
    let mut details = String::from(
        "SYNTHETIC proof identities; no source qualification or authentication occurred.\nProfile: oz570-solc0838-cancun-erc20-v1\nEvidence block: synthetic\n",
    );
    for index in 1..=16 {
        let _ = writeln!(
            details,
            "Synthetic technical detail {index}: 0x{}",
            "ab".repeat(32)
        );
    }
    let mut display = render(&ReviewBody {
        step_index: 0,
        steps: 2,
        network: "Base Sepolia · eip155:84532",
        account,
        target: "0x2222222222222222222222222222222222222222",
        action: &action,
        function: "approve(address,uint256)",
        arguments: &[serde_json::json!(account), serde_json::json!("100")],
        allowance: Some(&allowance),
        balances: &[],
        maximum_fee_eth: "0.0000018",
        maximum_fee_wei: "1800000000000",
        scope,
        details: &details,
        base: true,
        testnet: true,
    });
    display.body.insert_str(
        0,
        "SYNTHETIC DISPLAY FIXTURE — no transaction can be signed or sent\n\n",
    );
    display.final_disclaimer.push_str(
        "\nEND OF SYNTHETIC EVIDENCE — choices only close this fixture. No password is requested.",
    );
    display.model_summary = Some("Fixed preview example: set this account's token allowance to100 raw units. This illustrative text was not generated by a model.".into());
    display.title = "Synthetic contract review — display only".into();
    display
}

fn qualified_evidence(
    candidate: &QualifiedContractCandidate,
    scope: &app_core::ContractWorkflowScope,
    session_id: &str,
    expires_at_unix: u64,
    fees: TransferFeeBudget,
    semantic: &[u8],
) -> ApprovalEvidence {
    let intent = candidate.intent();
    let identity = candidate.identity();
    ApprovalEvidence {
        signing_digest: eip1559_signing_digest(candidate.payload()),
        chain_id: intent.network().descriptor().chain_id(),
        account: intent.account().into(),
        session_id: session_id.into(),
        expires_at_unix,
        transaction_type: ApprovalTransactionType::Eip1559 {
            transaction_type: 2,
        },
        decoder_digest: digest_parts(
            b"minutus.qualified-contract-source.v1",
            &[
                semantic,
                identity.profile_revision().as_bytes(),
                scope.commitment().as_bytes(),
            ],
        ),
        simulation_digest: digest_parts(
            b"minutus.qualified-contract-effects.v1",
            &[
                candidate.candidate_digest().as_bytes(),
                candidate.effect_commitment().as_bytes(),
                candidate.maximum_fee_wei().as_bytes(),
            ],
        ),
        fee_budget: fees,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn material_and_fee_facts_precede_technical_details_in_shared_renderer() -> Result<(), String> {
        let view = super::synthetic_preview();
        let body = view.complete_body().map_err(|error| error.message)?;
        let detail = body
            .find("TECHNICAL DETAILS")
            .ok_or("missing technical section")?;
        for material in [
            "ACTION:",
            "Spender:",
            "New allowance:",
            "ALLOWANCE",
            "MAXIMUM TOTAL NATIVE FEE",
            "0.0000018 ETH (1800000000000 wei)",
        ] {
            assert!(
                body.find(material).is_some_and(|offset| offset < detail),
                "{material}"
            );
        }
        assert!(body.contains("not an on-chain cap"));
        assert!(body.contains("END OF SYNTHETIC EVIDENCE"));
        assert!(body.find("MODEL SUMMARY") < body.find("TECHNICAL DETAILS"));
        assert!(body.find("TECHNICAL DETAILS") < body.find("DISCLAIMER:"));
        Ok(())
    }
}
