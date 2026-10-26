use std::error::Error;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use chain_evm::EvmNetwork;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use transaction_policy::{ApprovalCommitment, ApprovalCommitmentError};

use super::{
    MIN_X402_PREPARATION_LIFETIME_SECONDS, X402_PREPARATION_REVISION, prepare_with_nonce,
    types::{ActiveX402WalletAccount, PreparedFields, X402HttpMethod, X402PreparationError},
};
use crate::{
    PaymentRequiredChallenge, X402_POLICY_REVISION, admit_payment_requirement,
    parse_payment_required_response,
};

const RESOURCE: &str = "https://pay.flowzed.com/premium-data?market=eth";
const ASSET: &str = "0x036cbd53842c5426634e7929541ec2318f3dcf7e";
const RECIPIENT: &str = "0x209693bc6afc0c5328ba36faf03c514ef312287c";
const ACCOUNT: &str = "0x857b06519e91e3a54538791bdbb0e22373e36b66";
const RECEIVED_AT: u64 = 1_000;
const EXPIRES_AT: u64 = 1_060;
const REQUEST_ID: [u8; 32] = [0x42; 32];
const NONCE: [u8; 32] = [0x11; 32];

fn challenge(amount: &str, recipient: &str) -> Result<PaymentRequiredChallenge, Box<dyn Error>> {
    challenge_with_extensions(amount, recipient, &json!({}))
}

fn challenge_with_extensions(
    amount: &str,
    recipient: &str,
    extensions: &Value,
) -> Result<PaymentRequiredChallenge, Box<dyn Error>> {
    let value = json!({
        "x402Version": 2,
        "resource": {"url": RESOURCE},
        "accepts": [{
            "scheme": "exact",
            "network": "eip155:84532",
            "amount": amount,
            "asset": ASSET,
            "payTo": recipient,
            "maxTimeoutSeconds": 60,
            "extra": {"name": "USDC", "version": "2"}
        }],
        "extensions": extensions
    });
    let header = BASE64_STANDARD.encode(serde_json::to_vec(&value)?);
    Ok(parse_payment_required_response(
        402,
        &[header.as_bytes()],
        RESOURCE,
        RECEIVED_AT,
    )?)
}

#[test]
fn bazaar_changes_bind_approval_without_changing_the_payment_authorization()
-> Result<(), Box<dyn Error>> {
    let account = account(EvmNetwork::BaseSepolia)?;
    let prepare_metadata =
        |label: &str| -> Result<super::PreparedX402Authorization, Box<dyn Error>> {
            let challenge = challenge_with_extensions(
                "10000",
                RECIPIENT,
                &json!({"bazaar": {
                    "info": {"input": {"type": "http", "method": "GET", "queryParams": {"label": label}}},
                    "schema": {"type": "object"}
                }}),
            )?;
            let admitted = admit_payment_requirement(&challenge, 0, RECEIVED_AT + 1)?;
            Ok(prepare_with_nonce(
                &admitted,
                X402HttpMethod::Get,
                REQUEST_ID,
                Some(&account),
                RECEIVED_AT + 1,
                NONCE,
            )?)
        };
    let first = prepare_metadata("first")?;
    let same = prepare_metadata("first")?;
    let changed = prepare_metadata("changed")?;
    assert_eq!(first.candidate_digest(), same.candidate_digest());
    assert_ne!(first.candidate_digest(), changed.candidate_digest());
    assert_eq!(first.signing_digest(), changed.signing_digest());
    assert_eq!(first.canonical_typed_data(), changed.canonical_typed_data());
    let approved = ApprovalCommitment::issue(
        first.approval_binding_input("session", 1_050, [0x33; 32])?,
        1_001,
    )?;
    assert!(
        approved
            .verify(
                same.approval_binding_input("session", 1_050, [0x33; 32])?,
                1_002
            )
            .is_ok()
    );
    assert!(
        approved
            .verify(
                changed.approval_binding_input("session", 1_050, [0x33; 32])?,
                1_002
            )
            .is_err()
    );
    Ok(())
}

fn account(network: EvmNetwork) -> Result<ActiveX402WalletAccount, X402PreparationError> {
    ActiveX402WalletAccount::new(ACCOUNT, network)
}

fn prepare(
    amount: &str,
    recipient: &str,
    method: X402HttpMethod,
    request_id: [u8; 32],
    nonce: [u8; 32],
) -> Result<super::PreparedX402Authorization, Box<dyn Error>> {
    let challenge = challenge(amount, recipient)?;
    let admitted = admit_payment_requirement(&challenge, 0, RECEIVED_AT + 1)?;
    let account = account(EvmNetwork::BaseSepolia)?;
    Ok(prepare_with_nonce(
        &admitted,
        method,
        request_id,
        Some(&account),
        RECEIVED_AT + 1,
        nonce,
    )?)
}

#[test]
fn prepares_exact_request_bound_eip3009_candidate() -> Result<(), Box<dyn Error>> {
    let candidate = prepare("10000", RECIPIENT, X402HttpMethod::Get, REQUEST_ID, NONCE)?;

    assert_eq!(candidate.preparation_revision(), X402_PREPARATION_REVISION);
    assert_eq!(candidate.policy_revision(), X402_POLICY_REVISION);
    assert_eq!(candidate.requirement_index(), 0);
    assert_eq!(candidate.resource().canonical_url(), RESOURCE);
    assert_eq!(candidate.resource().origin(), "https://pay.flowzed.com");
    assert_eq!(candidate.method().as_str(), "GET");
    assert_eq!(candidate.request_identity_sha256(), &REQUEST_ID);
    assert_eq!(candidate.network(), EvmNetwork::BaseSepolia);
    assert_eq!(candidate.asset().token_contract(), Some(ASSET));

    let decoded = candidate.decoded();
    assert_eq!(decoded.domain_name(), "USDC");
    assert_eq!(decoded.domain_version(), "2");
    assert_eq!(decoded.chain_id(), 84_532);
    assert_eq!(decoded.verifying_contract(), ASSET);
    assert_eq!(decoded.account(), ACCOUNT);
    assert_eq!(decoded.recipient(), RECIPIENT);
    assert_eq!(decoded.amount(), "10000");
    assert_eq!(decoded.valid_after(), 0);
    assert_eq!(decoded.valid_before(), EXPIRES_AT);
    assert_eq!(decoded.nonce(), &NONCE);
    assert!(!candidate.is_expired_at(EXPIRES_AT - 1));
    assert!(candidate.is_expired_at(EXPIRES_AT));
    assert_ne!(candidate.signing_digest(), &[0; 32]);
    assert_ne!(candidate.candidate_digest(), &[0; 32]);
    Ok(())
}

#[test]
fn canonical_typed_data_and_signing_digest_are_deterministic() -> Result<(), Box<dyn Error>> {
    let first = prepare("10000", RECIPIENT, X402HttpMethod::Get, REQUEST_ID, NONCE)?;
    let second = prepare("10000", RECIPIENT, X402HttpMethod::Get, REQUEST_ID, NONCE)?;

    assert_eq!(first.canonical_typed_data(), second.canonical_typed_data());
    assert_eq!(first.signing_digest(), second.signing_digest());
    assert_eq!(first.candidate_digest(), second.candidate_digest());
    let value: Value = serde_json::from_str(first.canonical_typed_data())?;
    assert_eq!(value["primaryType"], "TransferWithAuthorization");
    assert_eq!(value["domain"]["chainId"], 84_532);
    assert_eq!(value["message"]["validAfter"], "0");
    assert_eq!(value["message"]["validBefore"], EXPIRES_AT.to_string());
    assert_eq!(value["message"]["nonce"], format!("0x{}", "11".repeat(32)));
    Ok(())
}

#[test]
fn every_request_or_payment_mutation_changes_candidate_identity() -> Result<(), Box<dyn Error>> {
    let baseline = prepare("10000", RECIPIENT, X402HttpMethod::Get, REQUEST_ID, NONCE)?;
    let changes = [
        prepare("10001", RECIPIENT, X402HttpMethod::Get, REQUEST_ID, NONCE)?,
        prepare(
            "10000",
            "0x0000000000000000000000000000000000000001",
            X402HttpMethod::Get,
            REQUEST_ID,
            NONCE,
        )?,
        prepare("10000", RECIPIENT, X402HttpMethod::Head, REQUEST_ID, NONCE)?,
        prepare("10000", RECIPIENT, X402HttpMethod::Get, [0x43; 32], NONCE)?,
        prepare(
            "10000",
            RECIPIENT,
            X402HttpMethod::Get,
            REQUEST_ID,
            [0x12; 32],
        )?,
    ];
    for changed in changes {
        assert_ne!(changed.candidate_digest(), baseline.candidate_digest());
    }
    Ok(())
}

#[test]
fn independent_decoder_rejects_reencoding_and_field_substitution() -> Result<(), Box<dyn Error>> {
    let candidate = prepare("10000", RECIPIENT, X402HttpMethod::Get, REQUEST_ID, NONCE)?;
    let mut value: Value = serde_json::from_str(candidate.canonical_typed_data())?;
    value["message"]["value"] = json!("10001");
    let changed = serde_json::to_string(&value)?;
    let fields = PreparedFields {
        account: ACCOUNT,
        recipient: RECIPIENT,
        amount: "10000",
        valid_after: 0,
        valid_before: EXPIRES_AT,
        nonce: NONCE,
        chain_id: 84_532,
        asset: ASSET,
    };
    assert_eq!(
        super::decode::decode_and_verify_authorization(&changed, &fields).err(),
        Some(X402PreparationError::CandidateMismatch)
    );

    let pretty = serde_json::to_string_pretty(&serde_json::from_str::<Value>(
        candidate.canonical_typed_data(),
    )?)?;
    assert_ne!(pretty, candidate.canonical_typed_data());
    let pretty_digest: [u8; 32] = Sha256::digest(pretty.as_bytes()).into();
    let canonical_digest: [u8; 32] =
        Sha256::digest(candidate.canonical_typed_data().as_bytes()).into();
    assert_ne!(pretty_digest, canonical_digest);
    Ok(())
}

#[test]
fn wallet_and_expiry_preconditions_fail_closed() -> Result<(), Box<dyn Error>> {
    let challenge = challenge("10000", RECIPIENT)?;
    let admitted = admit_payment_requirement(&challenge, 0, RECEIVED_AT + 1)?;
    assert_eq!(
        prepare_with_nonce(
            &admitted,
            X402HttpMethod::Get,
            REQUEST_ID,
            None,
            RECEIVED_AT + 1,
            NONCE,
        )
        .err(),
        Some(X402PreparationError::WalletUnavailable)
    );
    let wrong_network = account(EvmNetwork::BaseMainnet)?;
    assert_eq!(
        prepare_with_nonce(
            &admitted,
            X402HttpMethod::Get,
            REQUEST_ID,
            Some(&wrong_network),
            RECEIVED_AT + 1,
            NONCE,
        )
        .err(),
        Some(X402PreparationError::WalletNetworkMismatch)
    );
    let active = account(EvmNetwork::BaseSepolia)?;
    assert_eq!(
        prepare_with_nonce(
            &admitted,
            X402HttpMethod::Get,
            [0; 32],
            Some(&active),
            RECEIVED_AT + 1,
            NONCE,
        )
        .err(),
        Some(X402PreparationError::InvalidRequestIdentity)
    );
    assert_eq!(
        prepare_with_nonce(
            &admitted,
            X402HttpMethod::Get,
            REQUEST_ID,
            Some(&active),
            EXPIRES_AT,
            NONCE,
        )
        .err(),
        Some(X402PreparationError::ExpiredRequirement)
    );
    assert_eq!(
        prepare_with_nonce(
            &admitted,
            X402HttpMethod::Get,
            REQUEST_ID,
            Some(&active),
            EXPIRES_AT - MIN_X402_PREPARATION_LIFETIME_SECONDS + 1,
            NONCE,
        )
        .err(),
        Some(X402PreparationError::ExpiryTooNear)
    );
    assert_eq!(
        ActiveX402WalletAccount::new(
            "0x0000000000000000000000000000000000000000",
            EvmNetwork::BaseSepolia
        )
        .err(),
        Some(X402PreparationError::InvalidActivatedAccount)
    );
    Ok(())
}

#[test]
fn candidate_is_compatible_with_exact_epic_13_approval_binding() -> Result<(), Box<dyn Error>> {
    let candidate = prepare("10000", RECIPIENT, X402HttpMethod::Get, REQUEST_ID, NONCE)?;
    let input = candidate.approval_binding_input("x402-session-1", 1_050, [0x77; 32])?;
    let commitment = ApprovalCommitment::issue(input, 1_001)?;
    assert!(commitment.verify(input, 1_002).is_ok());

    let changed = candidate.approval_binding_input("x402-session-1", 1_050, [0x78; 32])?;
    assert_eq!(
        commitment.verify(changed, 1_002).err(),
        Some(ApprovalCommitmentError::SimulationMismatch)
    );
    assert_eq!(
        candidate
            .approval_binding_input("x402-session-1", EXPIRES_AT + 1, [0x77; 32])
            .err(),
        Some(X402PreparationError::ApprovalOutlivesAuthorization)
    );
    Ok(())
}

#[test]
fn mainnet_usdc_uses_its_own_domain_and_chain_and_rejects_testnet_terms()
-> Result<(), Box<dyn Error>> {
    let value = json!({
        "x402Version":2, "resource":{"url":RESOURCE},
        "accepts":[{"scheme":"exact", "network":"eip155:8453", "amount":"10000",
            "asset":"0x833589fcd6edb6e08f4c7c32d4f71b54bda02913", "payTo":RECIPIENT,
            "maxTimeoutSeconds":60, "extra":{"name":"USD Coin","version":"2"}}]
    });
    let header = BASE64_STANDARD.encode(serde_json::to_vec(&value)?);
    let challenge =
        parse_payment_required_response(402, &[header.as_bytes()], RESOURCE, RECEIVED_AT)?;
    let admitted = admit_payment_requirement(&challenge, 0, RECEIVED_AT + 1)?;
    assert_eq!(admitted.network(), EvmNetwork::BaseMainnet);
    let account = account(EvmNetwork::BaseMainnet)?;
    let prepared = prepare_with_nonce(
        &admitted,
        X402HttpMethod::Post,
        [0x42; 32],
        Some(&account),
        RECEIVED_AT + 1,
        [0x33; 32],
    )?;
    assert_eq!(prepared.decoded().domain_name(), "USD Coin");
    assert_eq!(prepared.decoded().chain_id(), 8453);
    let encoded: Value = serde_json::from_str(prepared.canonical_typed_data())?;
    assert_eq!(encoded["domain"]["name"], "USD Coin");
    assert_eq!(
        encoded["domain"]["verifyingContract"],
        "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"
    );
    super::revalidate_prepared_x402_authorization(&prepared, RECEIVED_AT + 2)?;
    let wrong_account = self::account(EvmNetwork::BaseSepolia)?;
    assert!(
        prepare_with_nonce(
            &admitted,
            X402HttpMethod::Post,
            [0x42; 32],
            Some(&wrong_account),
            RECEIVED_AT + 1,
            [0x33; 32]
        )
        .is_err()
    );
    for (field, replacement) in [("asset", ASSET), ("extra.name", "USDC")] {
        let mut wrong = value.clone();
        if field == "asset" {
            wrong["accepts"][0]["asset"] = json!(replacement);
        } else {
            wrong["accepts"][0]["extra"]["name"] = json!(replacement);
        }
        let header = BASE64_STANDARD.encode(serde_json::to_vec(&wrong)?);
        let challenge =
            parse_payment_required_response(402, &[header.as_bytes()], RESOURCE, RECEIVED_AT)?;
        assert!(admit_payment_requirement(&challenge, 0, RECEIVED_AT + 1).is_err());
    }
    Ok(())
}
