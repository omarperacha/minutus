//! Source-qualified material facts lead the native review; technical bindings
//! remain selectable below them. Presentation data can never mint approval.
use super::{ApprovalDisplay, ApprovalEvidence, PreparedApproval, digest_parts};
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
    let body = render(&ReviewBody {
        step_index,
        steps: scope.intent().steps.len(),
        network: &network,
        account: candidate.intent().account(),
        target: candidate.identity().target(),
        action: candidate.action(),
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
        display: ApprovalDisplay {
            title: "Review source-qualified contract write".into(),
            body,
            has_warnings: true,
            approval_allowed: true,
        },
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
    allowance: Option<&'a AllowanceEffect>,
    balances: &'a [TokenBalanceEffect],
    maximum_fee_eth: &'a str,
    maximum_fee_wei: &'a str,
    scope: &'a str,
    details: &'a str,
    base: bool,
    testnet: bool,
}
fn render(view: &ReviewBody<'_>) -> String {
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
        "\nNATIVE VALUE: 0 wei\nMAXIMUM TOTAL NATIVE FEE\n{} ETH ({} wei)",
        view.maximum_fee_eth, view.maximum_fee_wei
    );
    body.push_str("\nEach write requires its own approval and fresh password. An allowance may remain if a later step fails. Receipt success alone does not complete the task. No automatic cleanup or retry.\n");
    if view.base {
        body.push_str("\nBASE: Material effects are qualified before signing. The genuine signed transaction must pass the isolated native handler before submission. This fee budget is checked against current evidence; Base L1 inclusion/operator charges are not an on-chain cap in the signed transaction.\n");
    }
    let _ = write!(
        body,
        "\nFULL IMMUTABLE ORDERED SCOPE\n{}\n\nTECHNICAL EVIDENCE AND EXACT TRANSACTION\n{}",
        view.scope, view.details
    );
    body
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
#[cfg(any(debug_assertions, test))]
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
    let mut body = render(&ReviewBody {
        step_index: 0,
        steps: 2,
        network: "Base Sepolia · eip155:84532",
        account,
        target: "0x2222222222222222222222222222222222222222",
        action: &action,
        allowance: Some(&allowance),
        balances: &[],
        maximum_fee_eth: "0.0000018",
        maximum_fee_wei: "1800000000000",
        scope,
        details: &details,
        base: true,
        testnet: true,
    });
    body.insert_str(
        0,
        "SYNTHETIC DISPLAY FIXTURE — no transaction can be signed or sent\n\n",
    );
    body.push_str("\nEND OF SYNTHETIC EVIDENCE — choices only close this fixture. No password is requested.\n");
    ApprovalDisplay {
        title: "Synthetic contract review — display only".into(),
        body,
        has_warnings: true,
        approval_allowed: true,
    }
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
    fn material_and_fee_facts_precede_technical_details_in_shared_renderer() {
        let view = super::synthetic_preview();
        let detail = view.body.find("TECHNICAL EVIDENCE").unwrap_or(0);
        for material in [
            "ACTION:",
            "Spender:",
            "New allowance:",
            "ALLOWANCE",
            "MAXIMUM TOTAL NATIVE FEE",
            "0.0000018 ETH (1800000000000 wei)",
        ] {
            assert!(
                view.body
                    .find(material)
                    .is_some_and(|offset| offset < detail),
                "{material}"
            );
        }
        assert!(view.body.contains("not an on-chain cap"));
        assert!(view.body.contains("END OF SYNTHETIC EVIDENCE"));
    }
}
