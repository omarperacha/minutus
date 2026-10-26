use std::error::Error;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use chain_evm::EvmNetwork;
use serde_json::{Value, json};

use super::{
    X402PaymentError, create_x402_payment_signature_header, validate_x402_payment_response,
};
use crate::{
    ActiveX402WalletAccount, X402HttpMethod, admit_payment_requirement,
    parse_payment_required_response, prepare_x402_payment_authorization,
};

const URL: &str = "https://pay.flowzed.com/data?item=eth";
const ASSET: &str = "0x036cbd53842c5426634e7929541ec2318f3dcf7e";
const PAYEE: &str = "0x209693bc6afc0c5328ba36faf03c514ef312287c";
const PAYER: &str = "0x857b06519e91e3a54538791bdbb0e22373e36b66";

fn candidate() -> Result<crate::PreparedX402Authorization, Box<dyn Error>> {
    candidate_with_extensions(&json!({}))
}

fn candidate_with_extensions(
    extensions: &Value,
) -> Result<crate::PreparedX402Authorization, Box<dyn Error>> {
    let challenge = json!({
        "x402Version": 2,
        "resource": {"url": URL},
        "accepts": [{
            "scheme": "exact", "network": "eip155:84532", "amount": "10000",
            "asset": ASSET, "payTo": PAYEE, "maxTimeoutSeconds": 60,
            "extra": {"name": "USDC", "version": "2"}
        }],
        "extensions": extensions
    });
    let header = STANDARD.encode(serde_json::to_vec(&challenge)?);
    let challenge = parse_payment_required_response(402, &[header.as_bytes()], URL, 1_000)?;
    let admitted = admit_payment_requirement(&challenge, 0, 1_001)?;
    let account = ActiveX402WalletAccount::new(PAYER, EvmNetwork::BaseSepolia)?;
    Ok(prepare_x402_payment_authorization(
        &admitted,
        X402HttpMethod::Get,
        [0x42; 32],
        Some(&account),
        1_001,
    )?)
}

#[test]
fn bazaar_metadata_survives_admission_preparation_and_payment_header() -> Result<(), Box<dyn Error>>
{
    let extensions = json!({"bazaar": {
        "info": {"input": {"type": "http", "method": "GET", "queryParams": {"item": "eth"}}},
        "schema": {"type": "object", "properties": {"input": {"type": "object"}}, "required": ["input"]}
    }});
    let candidate = candidate_with_extensions(&extensions)?;
    crate::revalidate_prepared_x402_authorization(&candidate, 1_002)?;
    let header = create_x402_payment_signature_header(&candidate, &[1; 32], &[2; 32], false)?;
    let value: Value = serde_json::from_slice(&STANDARD.decode(header.as_str())?)?;
    assert_eq!(value["extensions"], extensions);
    assert_eq!(value["accepted"]["amount"], "10000");
    assert_eq!(value["accepted"]["asset"], ASSET);
    assert_eq!(value["payload"]["authorization"]["to"], PAYEE);
    Ok(())
}

#[test]
fn payment_header_is_canonical_v2_exact_eip3009() -> Result<(), Box<dyn Error>> {
    let candidate = candidate()?;
    let header = create_x402_payment_signature_header(&candidate, &[1; 32], &[2; 32], false)?;
    let decoded = STANDARD.decode(header.as_str())?;
    let value: Value = serde_json::from_slice(&decoded)?;

    assert_eq!(
        value.as_object().map(|fields| {
            let mut keys = fields.keys().map(String::as_str).collect::<Vec<_>>();
            keys.sort_unstable();
            keys
        }),
        Some(vec!["accepted", "payload", "resource", "x402Version"])
    );
    assert_eq!(value["x402Version"], 2);
    assert_eq!(value["resource"]["url"], URL);
    assert_eq!(value["accepted"]["network"], "eip155:84532");
    assert_eq!(value["accepted"]["amount"], "10000");
    assert_eq!(value["accepted"]["maxTimeoutSeconds"], 60);
    assert_eq!(value["payload"]["authorization"]["from"], PAYER);
    assert_eq!(value["payload"]["authorization"]["to"], PAYEE);
    assert_eq!(
        value["payload"]["signature"].as_str().map(str::len),
        Some(132)
    );
    assert!(
        value["payload"]["signature"]
            .as_str()
            .is_some_and(|value| value.ends_with("1b"))
    );
    Ok(())
}

#[test]
fn validates_response_against_network_payer_and_amount() -> Result<(), Box<dyn Error>> {
    let candidate = candidate()?;
    let response = json!({
        "success": true,
        "transaction": format!("0x{}", "ab".repeat(32)),
        "network": "eip155:84532",
        "payer": PAYER,
        "amount": "10000"
    });
    let header = STANDARD.encode(serde_json::to_vec(&response)?);
    let validated = validate_x402_payment_response(200, &[header.as_bytes()], &candidate)?;
    assert_eq!(validated.network(), "eip155:84532");
    assert_eq!(validated.payer(), PAYER);
    assert_eq!(validated.amount(), Some("10000"));
    assert_eq!(validated.transaction(), format!("0x{}", "ab".repeat(32)));
    Ok(())
}

#[test]
fn missing_duplicate_failed_and_mismatched_responses_fail_closed() -> Result<(), Box<dyn Error>> {
    let candidate = candidate()?;
    assert_eq!(
        validate_x402_payment_response(200, &[], &candidate).err(),
        Some(X402PaymentError::MissingResponse)
    );
    assert_eq!(
        validate_x402_payment_response(500, &[], &candidate).err(),
        Some(X402PaymentError::UnexpectedStatus)
    );
    let failed = STANDARD.encode(br#"{"success":false,"errorReason":"settlement_pending","transaction":"0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","network":"eip155:84532"}"#);
    assert_eq!(
        validate_x402_payment_response(200, &[failed.as_bytes()], &candidate).err(),
        Some(X402PaymentError::SettlementFailed)
    );
    let mismatch = STANDARD.encode(serde_json::to_vec(&json!({
        "success": true, "transaction": format!("0x{}", "ab".repeat(32)),
        "network": "eip155:8453", "payer": PAYER
    }))?);
    assert_eq!(
        validate_x402_payment_response(200, &[mismatch.as_bytes()], &candidate).err(),
        Some(X402PaymentError::CandidateMismatch)
    );
    assert_eq!(
        validate_x402_payment_response(
            200,
            &[mismatch.as_bytes(), mismatch.as_bytes()],
            &candidate
        )
        .err(),
        Some(X402PaymentError::DuplicateResponse)
    );
    Ok(())
}

#[test]
fn duplicate_keys_and_unknown_fields_are_ambiguous() -> Result<(), Box<dyn Error>> {
    let candidate = candidate()?;
    let duplicate = STANDARD.encode(format!(
        r#"{{"success":true,"success":true,"transaction":"0x{}","network":"eip155:84532","payer":"{PAYER}"}}"#,
        "ab".repeat(32)
    ));
    assert_eq!(
        validate_x402_payment_response(200, &[duplicate.as_bytes()], &candidate).err(),
        Some(X402PaymentError::MalformedResponse)
    );
    Ok(())
}
