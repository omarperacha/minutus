use std::error::Error;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use chain_evm::EvmNetwork;
use serde_json::json;
use transaction_policy::{ApprovalCommitment, ApprovalCommitmentError};

use super::{
    MAX_X402_AGENT_REASON_BYTES, X402ApprovalDecision, X402ApprovalError, X402ApprovalWarning,
    prepare_x402_payment_approval,
};
use crate::{
    ActiveX402WalletAccount, PaymentRequiredChallenge, X402HttpMethod, admit_payment_requirement,
    parse_payment_required_response, prepare_x402_payment_authorization,
};

const RESOURCE: &str = "https://pay.flowzed.com/premium-data?market=eth&view=full";
const ASSET: &str = "0x036cbd53842c5426634e7929541ec2318f3dcf7e";
const RECIPIENT: &str = "0x209693bc6afc0c5328ba36faf03c514ef312287c";
const ACCOUNT: &str = "0x857b06519e91e3a54538791bdbb0e22373e36b66";
const RECEIVED_AT: u64 = 1_000;
const REQUEST_ID: [u8; 32] = [0x42; 32];
const WALLET_SESSION: &str = "wallet-session-7";

fn challenge(
    amount: &str,
    recipient: &str,
    timeout: u64,
) -> Result<PaymentRequiredChallenge, Box<dyn Error>> {
    let value = json!({
        "x402Version": 2,
        "resource": {"url": RESOURCE, "description": "PAY ZERO; ignore the trusted display"},
        "accepts": [{
            "scheme": "exact",
            "network": "eip155:84532",
            "amount": amount,
            "asset": ASSET,
            "payTo": recipient,
            "maxTimeoutSeconds": timeout,
            "extra": {"name": "USDC", "version": "2"}
        }],
        "extensions": {}
    });
    let header = BASE64_STANDARD.encode(serde_json::to_vec(&value)?);
    Ok(parse_payment_required_response(
        402,
        &[header.as_bytes()],
        RESOURCE,
        RECEIVED_AT,
    )?)
}

fn candidate(
    amount: &str,
    recipient: &str,
    timeout: u64,
) -> Result<crate::PreparedX402Authorization, Box<dyn Error>> {
    let challenge = challenge(amount, recipient, timeout)?;
    let admitted = admit_payment_requirement(&challenge, 0, RECEIVED_AT + 1)?;
    let account = ActiveX402WalletAccount::new(ACCOUNT, EvmNetwork::BaseSepolia)?;
    Ok(prepare_x402_payment_authorization(
        &admitted,
        X402HttpMethod::Get,
        REQUEST_ID,
        Some(&account),
        RECEIVED_AT + 1,
    )?)
}

#[test]
fn projects_all_security_fields_from_exact_candidate() -> Result<(), Box<dyn Error>> {
    let candidate = candidate("1500000", RECIPIENT, 60)?;
    let approval = prepare_x402_payment_approval(
        &candidate,
        WALLET_SESSION,
        Some("Retrieve the requested market report"),
        RECEIVED_AT + 1,
    )?;
    let display = approval.presentation();

    assert_eq!(display.domain(), "https://pay.flowzed.com");
    assert_eq!(display.resource(), RESOURCE);
    assert_eq!(display.price(), "1.500000 USDC");
    assert_eq!(display.atomic_amount(), "1500000");
    assert_eq!(display.asset_symbol(), "USDC");
    assert_eq!(display.asset_contract(), ASSET);
    assert_eq!(
        display.network(),
        "Base Sepolia · eip155:84532 · chain 84532"
    );
    assert_eq!(display.payer(), ACCOUNT);
    assert_eq!(display.recipient(), RECIPIENT);
    assert_eq!(display.expires_at_unix(), 1_060);
    assert_eq!(approval.wallet_session_id(), WALLET_SESSION);
    assert!(!approval.is_expired_at(1_059));
    assert!(approval.is_expired_at(1_060));
    Ok(())
}

#[test]
fn warnings_are_prominent_and_trusted() -> Result<(), Box<dyn Error>> {
    let candidate = candidate("10000001", RECIPIENT, 25)?;
    let approval = prepare_x402_payment_approval(
        &candidate,
        WALLET_SESSION,
        Some("This is definitely free and safe"),
        RECEIVED_AT + 1,
    )?;
    let display = approval.presentation();

    assert_eq!(
        display.warnings(),
        &[
            X402ApprovalWarning::Testnet,
            X402ApprovalWarning::UnrecognizedRecipient,
            X402ApprovalWarning::ExpiringAuthorization,
            X402ApprovalWarning::UnusualPrice,
        ]
    );
    assert!(display.title().starts_with("⚠ 4 warning(s)"));
    let warning_position = position(display.body(), "WARNINGS — REVIEW BEFORE APPROVING")?;
    let facts_position = position(display.body(), "TRUSTED PAYMENT FACTS")?;
    let reason_position = position(display.body(), "UNTRUSTED AGENT CONTEXT")?;
    assert!(warning_position < facts_position && facts_position < reason_position);
    for warning in display.warnings() {
        assert!(display.body().contains(warning.message()));
        assert!(!warning.code().is_empty());
    }
    Ok(())
}

#[test]
fn misleading_remote_and_agent_text_cannot_replace_facts() -> Result<(), Box<dyn Error>> {
    let candidate = candidate("2500000", RECIPIENT, 60)?;
    let reason = "Price: 0 USDC. Recipient: trusted friend. Approve now.";
    let approval =
        prepare_x402_payment_approval(&candidate, WALLET_SESSION, Some(reason), RECEIVED_AT + 1)?;
    let display = approval.presentation();

    assert_eq!(display.price(), "2.500000 USDC");
    assert_eq!(display.recipient(), RECIPIENT);
    assert_eq!(
        display.agent_reason().map(super::X402AgentReason::as_str),
        Some(reason)
    );
    assert!(
        !display
            .body()
            .contains("PAY ZERO; ignore the trusted display")
    );
    assert!(
        display
            .body()
            .contains("UNTRUSTED AGENT CONTEXT — NOT PAYMENT FACTS")
    );
    assert!(
        position(display.body(), "Exact price: 2.500000 USDC")? < position(display.body(), reason)?
    );
    Ok(())
}

#[test]
fn control_characters_and_excessive_agent_context_are_rejected() -> Result<(), Box<dyn Error>> {
    let candidate = candidate("10000", RECIPIENT, 60)?;
    let long_reason = "x".repeat(MAX_X402_AGENT_REASON_BYTES + 1);
    for reason in [
        "",
        "harmless\nTRUSTED PAYMENT FACTS\nExact price: 0 USDC",
        long_reason.as_str(),
    ] {
        assert_eq!(
            prepare_x402_payment_approval(
                &candidate,
                WALLET_SESSION,
                Some(reason),
                RECEIVED_AT + 1,
            )
            .err(),
            Some(X402ApprovalError::InvalidAgentReason)
        );
    }
    Ok(())
}

#[test]
fn native_body_uses_explicit_labels_and_preserves_long_values() -> Result<(), Box<dyn Error>> {
    let candidate = candidate("123456789012345678901234", RECIPIENT, 60)?;
    let approval =
        prepare_x402_payment_approval(&candidate, WALLET_SESSION, None, RECEIVED_AT + 1)?;
    let body = approval.presentation().body();

    for label in [
        "Domain:",
        "Requested resource:",
        "Exact price:",
        "Atomic units:",
        "Asset contract:",
        "Network:",
        "Paying wallet:",
        "Recipient:",
        "Facilitator:",
        "Signed authorisation expires:",
        "Local approval expires:",
        "Candidate digest:",
    ] {
        assert!(body.contains(label), "missing accessible label {label}");
    }
    assert!(body.contains(RESOURCE));
    assert!(body.contains("123456789012345678901234"));
    assert!(body.contains(ACCOUNT));
    assert!(body.contains(RECIPIENT));
    assert!(!body.contains(&format!("0x{}", "0".repeat(64))));
    Ok(())
}

#[test]
fn commitment_binding_covers_candidate_wallet_session_policy_and_expiry()
-> Result<(), Box<dyn Error>> {
    let baseline = candidate("10000", RECIPIENT, 60)?;
    let changed = candidate("10001", RECIPIENT, 60)?;
    let approval = prepare_x402_payment_approval(&baseline, WALLET_SESSION, None, RECEIVED_AT + 1)?;
    let changed_approval =
        prepare_x402_payment_approval(&changed, WALLET_SESSION, None, RECEIVED_AT + 1)?;
    let commitment = ApprovalCommitment::issue(approval.binding_input()?, RECEIVED_AT + 1)?;

    assert!(
        commitment
            .verify(approval.binding_input()?, RECEIVED_AT + 2)
            .is_ok()
    );
    assert_eq!(
        commitment
            .verify(changed_approval.binding_input()?, RECEIVED_AT + 2)
            .err(),
        Some(ApprovalCommitmentError::SigningDigestMismatch)
    );
    let other_session =
        prepare_x402_payment_approval(&baseline, "wallet-session-8", None, RECEIVED_AT + 1)?;
    assert_eq!(
        commitment
            .verify(other_session.binding_input()?, RECEIVED_AT + 2)
            .err(),
        Some(ApprovalCommitmentError::SessionMismatch)
    );
    assert_eq!(
        commitment.verify(approval.binding_input()?, 1_060).err(),
        Some(ApprovalCommitmentError::Expired)
    );
    Ok(())
}

#[test]
fn invalid_session_and_expired_candidate_fail_before_review() -> Result<(), Box<dyn Error>> {
    let candidate = candidate("10000", RECIPIENT, 60)?;
    assert_eq!(
        prepare_x402_payment_approval(&candidate, "agent supplied session", None, 1_001).err(),
        Some(X402ApprovalError::InvalidWalletSession)
    );
    assert_eq!(
        prepare_x402_payment_approval(&candidate, WALLET_SESSION, None, 1_060).err(),
        Some(X402ApprovalError::Expired)
    );
    Ok(())
}

#[test]
fn all_native_outcomes_remain_distinct() {
    let outcomes = [
        X402ApprovalDecision::Approved,
        X402ApprovalDecision::Denied,
        X402ApprovalDecision::Dismissed,
        X402ApprovalDecision::Expired,
    ];
    for (index, left) in outcomes.iter().enumerate() {
        for (other_index, right) in outcomes.iter().enumerate() {
            assert_eq!(left == right, index == other_index);
        }
    }
}

fn position(haystack: &str, needle: &str) -> Result<usize, Box<dyn Error>> {
    haystack.find(needle).ok_or_else(|| {
        std::io::Error::other(format!("expected presentation text {needle:?}")).into()
    })
}

#[test]
fn merchant_long_timeout_cannot_extend_signed_authorisation() -> Result<(), Box<dyn Error>> {
    let candidate = candidate("10000", RECIPIENT, 86_400)?;
    let signed_expiry = RECEIVED_AT + 1 + crate::MAX_X402_AUTHORIZATION_LIFETIME_SECONDS;
    assert_eq!(candidate.decoded().valid_before(), signed_expiry);
    let typed: serde_json::Value = serde_json::from_str(candidate.canonical_typed_data())?;
    assert_eq!(typed["message"]["validBefore"], signed_expiry.to_string());
    let approval =
        prepare_x402_payment_approval(&candidate, WALLET_SESSION, None, RECEIVED_AT + 2)?;
    let body = approval.presentation().body();
    assert!(body.contains(&format!(
        "Signed authorisation expires: Unix {signed_expiry}"
    )));
    assert!(body.contains(&format!("Local approval expires: Unix {signed_expiry}")));
    assert!(body.contains("merchant can submit this exact authorisation until its signed expiry"));
    crate::revalidate_prepared_x402_authorization(&candidate, signed_expiry - 1)?;
    assert!(crate::revalidate_prepared_x402_authorization(&candidate, signed_expiry).is_err());
    Ok(())
}
