use super::{X402ApprovalControl, outcome};
#[cfg(target_os = "macos")]
use super::{X402ExecutionError, classify_payment_response_failure, classify_retry_failure};
#[cfg(target_os = "macos")]
use base64::{Engine as _, engine::general_purpose::STANDARD};
#[cfg(target_os = "macos")]
use chain_evm::EvmNetwork;
use ipc_types::{TransactionProposalId, X402ApprovalDecision};
#[cfg(target_os = "macos")]
use serde_json::json;
#[cfg(target_os = "macos")]
use transaction_policy::ApprovalCommitment;
#[cfg(target_os = "macos")]
use web_retrieval::{TargetValidationError, WebBrokerError};
#[cfg(target_os = "macos")]
use x402_client::{
    ActiveX402WalletAccount, X402HttpMethod, X402PaymentError, admit_payment_requirement,
    parse_payment_required_response, prepare_x402_payment_approval,
    prepare_x402_payment_authorization,
};

#[test]
fn typed_receipts_preserve_every_terminal_decision() {
    for decision in [
        X402ApprovalDecision::Approved,
        X402ApprovalDecision::Denied,
        X402ApprovalDecision::Dismissed,
        X402ApprovalDecision::Expired,
    ] {
        let receipt = outcome(TransactionProposalId("review-1".to_owned()), decision);
        assert_eq!(receipt.decision, decision);
        assert_eq!(receipt.review_id.0, "review-1");
    }
}

#[test]
fn unknown_review_references_fail_closed() -> Result<(), Box<dyn std::error::Error>> {
    let control = X402ApprovalControl::new();
    let error = control
        .begin(&TransactionProposalId("agent-authored".to_owned()))
        .err()
        .ok_or_else(|| std::io::Error::other("unknown reference opened native review"))?;
    assert_eq!(error.code, ipc_types::IpcErrorCode::InvalidRequest);
    Ok(())
}

#[test]
fn native_surface_defaults_to_denial_and_names_exact_payment() {
    let source = include_str!("native.rs");
    let deny = source.find("addButtonWithTitle(&NSString::from_str(\"Deny\"))");
    let approve = source.find("addButtonWithTitle(&NSString::from_str(\"Approve exact payment\"))");
    let dismiss = source.find("addButtonWithTitle(&NSString::from_str(\"Dismiss\"))");

    assert!(deny.is_some_and(|deny| approve.is_some_and(|approve| deny < approve)));
    assert!(approve.is_some_and(|approve| dismiss.is_some_and(|dismiss| approve < dismiss)));
    assert!(source.contains("NSAlertStyle::Warning"));
    assert!(source.contains("NSAlertFirstButtonReturn => X402ApprovalDecision::Denied"));
    assert!(source.contains("NSAlertSecondButtonReturn"));
}

#[cfg(target_os = "macos")]
#[test]
fn retry_failures_distinguish_confirmed_unpaid_from_ambiguous() {
    for (error, code) in [
        (WebBrokerError::DnsDeadline, "dns_timeout_unpaid"),
        (
            WebBrokerError::TargetRejected(TargetValidationError::NonPublicAddress),
            "target_rejected_unpaid",
        ),
        (
            WebBrokerError::ClientUnavailable,
            "client_unavailable_unpaid",
        ),
    ] {
        let failure = classify_retry_failure(error);
        assert_eq!(failure.code(), code);
        assert!(!failure.payment_may_have_occurred());
        assert!(!failure.payment_confirmed());
        assert!(!failure.automatic_retry_allowed());
        assert!(matches!(failure, X402ExecutionError::ConfirmedUnpaid(_)));
    }

    for error in [
        WebBrokerError::Cancelled,
        WebBrokerError::ConnectionOrHeaderDeadline,
        WebBrokerError::BodyIdleDeadline,
        WebBrokerError::TotalDeadline,
        WebBrokerError::ResponseRejected(
            web_retrieval::ResponseValidationError::UnexpectedPartialResponse,
        ),
        WebBrokerError::TransportFailed,
    ] {
        let failure = classify_retry_failure(error);
        assert!(failure.payment_may_have_occurred());
        assert!(!failure.automatic_retry_allowed());
        assert!(matches!(failure, X402ExecutionError::Ambiguous(_)));
        assert!(failure.to_string().contains("may have occurred"));
    }
}

#[cfg(target_os = "macos")]
#[test]
fn every_payment_response_failure_is_terminal_and_ambiguous() {
    let cases = [
        (
            X402PaymentError::UnexpectedStatus,
            "unexpected_status_ambiguous",
        ),
        (
            X402PaymentError::MissingResponse,
            "missing_payment_response_ambiguous",
        ),
        (
            X402PaymentError::DuplicateResponse,
            "duplicate_payment_response_ambiguous",
        ),
        (
            X402PaymentError::MalformedResponse,
            "malformed_payment_response_ambiguous",
        ),
        (
            X402PaymentError::SettlementFailed,
            "settlement_rejected_ambiguous",
        ),
        (
            X402PaymentError::UnsupportedExtension,
            "settlement_extension_ambiguous",
        ),
        (
            X402PaymentError::CandidateMismatch,
            "settlement_mismatch_ambiguous",
        ),
    ];
    for (error, code) in cases {
        let failure = classify_payment_response_failure(error);
        assert_eq!(failure.code(), code);
        assert!(failure.payment_may_have_occurred());
        assert!(!failure.requires_fresh_approval());
        assert!(!failure.automatic_retry_allowed());
    }
}

#[cfg(target_os = "macos")]
#[test]
fn changed_requirements_retain_only_a_fresh_non_authoritative_challenge()
-> Result<(), Box<dyn std::error::Error>> {
    let challenge = parsed_challenge("20000", 60)?;
    let failure = X402ExecutionError::RequirementsChanged(challenge);

    assert_eq!(failure.code(), "requirements_changed");
    assert!(failure.payment_may_have_occurred());
    assert!(failure.requires_fresh_approval());
    assert!(!failure.automatic_retry_allowed());
    assert_eq!(
        failure
            .fresh_challenge()
            .map(x402_client::PaymentRequiredChallenge::version),
        Some(2)
    );
    assert!(!format!("{failure:?}").contains("pay.flowzed.com"));
    Ok(())
}

#[cfg(target_os = "macos")]
#[test]
fn confirmed_payment_with_missing_content_is_not_ambiguous_or_retryable() {
    let failure = X402ExecutionError::PaymentConfirmedContentUnavailable;
    assert!(failure.payment_confirmed());
    assert!(!failure.payment_may_have_occurred());
    assert!(!failure.automatic_retry_allowed());
    assert!(failure.to_string().contains("payment succeeded"));
}

#[cfg(target_os = "macos")]
#[test]
fn concurrent_and_restart_delivery_cannot_reuse_an_approved_entry()
-> Result<(), Box<dyn std::error::Error>> {
    let control = std::sync::Arc::new(X402ApprovalControl::new());
    let review_id = TransactionProposalId("review-concurrent-1".to_owned());
    let approval = prepared_approval()?;
    let commitment = ApprovalCommitment::issue(approval.binding_input()?, 1_001)?;
    control
        .retain_approved(&review_id, approval, commitment)
        .map_err(|_| std::io::Error::other("could not retain approved payment"))?;

    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let successes = (0..8)
        .map(|_| {
            let control = std::sync::Arc::clone(&control);
            let barrier = std::sync::Arc::clone(&barrier);
            let review_id = review_id.clone();
            std::thread::spawn(move || {
                barrier.wait();
                control.take_approved(&review_id).is_ok()
            })
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|thread| thread.join().unwrap_or(false))
        .collect::<Vec<_>>();
    assert_eq!(successes.into_iter().filter(|success| *success).count(), 1);

    drop(control);
    let restarted = X402ApprovalControl::new();
    assert!(restarted.take_approved(&review_id).is_err());
    Ok(())
}

#[cfg(target_os = "macos")]
fn parsed_challenge(
    amount: &str,
    timeout: u64,
) -> Result<x402_client::PaymentRequiredChallenge, Box<dyn std::error::Error>> {
    const RESOURCE: &str = "https://pay.flowzed.com/premium-data";
    let value = json!({
        "x402Version": 2,
        "resource": {"url": RESOURCE},
        "accepts": [{
            "scheme": "exact",
            "network": "eip155:84532",
            "amount": amount,
            "asset": "0x036cbd53842c5426634e7929541ec2318f3dcf7e",
            "payTo": "0x209693bc6afc0c5328ba36faf03c514ef312287c",
            "maxTimeoutSeconds": timeout,
            "extra": {"name": "USDC", "version": "2"}
        }],
        "extensions": {}
    });
    let header = STANDARD.encode(serde_json::to_vec(&value)?);
    Ok(parse_payment_required_response(
        402,
        &[header.as_bytes()],
        RESOURCE,
        1_000,
    )?)
}

#[cfg(target_os = "macos")]
fn prepared_approval() -> Result<x402_client::PreparedX402Approval, Box<dyn std::error::Error>> {
    let challenge = parsed_challenge("10000", 60)?;
    let requirement = admit_payment_requirement(&challenge, 0, 1_001)?;
    let account = ActiveX402WalletAccount::new(
        "0x857b06519e91e3a54538791bdbb0e22373e36b66",
        EvmNetwork::BaseSepolia,
    )?;
    let candidate = prepare_x402_payment_authorization(
        &requirement,
        X402HttpMethod::Get,
        [0x42; 32],
        Some(&account),
        1_001,
    )?;
    Ok(prepare_x402_payment_approval(
        &candidate,
        "wallet-session-7",
        None,
        1_001,
    )?)
}
