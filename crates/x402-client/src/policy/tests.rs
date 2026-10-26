use std::error::Error;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use chain_evm::EvmNetwork;
use serde_json::{Value, json};

use super::{
    SupportedX402Scheme, X402_POLICY_REVISION, X402PolicyError, admit_payment_requirement,
    validate_x402_payment_terms,
};
#[cfg(debug_assertions)]
use super::{X402OriginPolicy, admit_debug_loopback_payment_requirement};
use crate::{PaymentRequiredChallenge, parse_payment_required_response};

const RESOURCE_URL: &str = "https://pay.flowzed.com/premium-data?market=eth";
const ASSET: &str = "0x036cbd53842c5426634e7929541ec2318f3dcf7e";
const RECIPIENT: &str = "0x209693bc6afc0c5328ba36faf03c514ef312287c";

fn valid_challenge() -> Value {
    json!({
        "x402Version": 2,
        "resource": {"url": RESOURCE_URL},
        "accepts": [{
            "scheme": "exact",
            "network": "eip155:84532",
            "amount": "10000",
            "asset": ASSET,
            "payTo": RECIPIENT,
            "maxTimeoutSeconds": 60,
            "extra": {"name": "USDC", "version": "2"}
        }],
        "extensions": {}
    })
}

fn parse(value: &Value, original_url: &str) -> Result<PaymentRequiredChallenge, Box<dyn Error>> {
    let encoded = BASE64_STANDARD.encode(serde_json::to_vec(value)?);
    Ok(parse_payment_required_response(
        402,
        &[encoded.as_bytes()],
        original_url,
        1_000,
    )?)
}

fn policy_error(value: &Value, original_url: &str) -> Result<X402PolicyError, Box<dyn Error>> {
    let challenge = parse(value, original_url)?;
    admit_payment_requirement(&challenge, 0, 1_001)
        .err()
        .ok_or_else(|| "expected policy rejection".into())
}

#[test]
fn exact_base_sepolia_usdc_is_admitted_as_public_policy_evidence() -> Result<(), Box<dyn Error>> {
    let challenge = parse(&valid_challenge(), RESOURCE_URL)?;
    let admitted = admit_payment_requirement(&challenge, 0, 1_001)?;

    assert_eq!(admitted.policy_revision(), X402_POLICY_REVISION);
    assert_eq!(admitted.requirement_index(), 0);
    assert_eq!(admitted.resource().origin(), "https://pay.flowzed.com");
    assert_eq!(admitted.scheme(), SupportedX402Scheme::ExactEip3009);
    assert_eq!(admitted.scheme().as_str(), "exact");
    assert_eq!(admitted.scheme().asset_transfer_method(), "eip3009");
    assert_eq!(admitted.scheme().payment_flow(), "authorization");
    assert_eq!(admitted.network(), EvmNetwork::BaseSepolia);
    assert_eq!(admitted.network().descriptor().chain_id(), 84_532);
    assert_eq!(admitted.asset().symbol(), "USDC");
    assert_eq!(admitted.asset().decimals(), 6);
    assert_eq!(admitted.asset().token_contract(), Some(ASSET));
    assert_eq!(admitted.amount(), "10000");
    assert_eq!(admitted.recipient(), RECIPIENT);
    assert_eq!(admitted.max_timeout_seconds(), 60);
    assert_eq!(admitted.expires_at_unix_seconds(), 1_060);
    assert!(!admitted.is_expired_at(1_059));
    assert!(admitted.is_expired_at(1_060));
    assert_eq!(
        X402PolicyError::UnsupportedNetwork.code(),
        "unsupported_network"
    );
    Ok(())
}

#[test]
fn term_inspection_never_admits_a_loopback_payment() -> Result<(), Box<dyn Error>> {
    let local_url = "http://127.0.0.1:5001/resources/run-1";
    let mut value = valid_challenge();
    value["resource"]["url"] = json!(local_url);
    let challenge = parse(&value, local_url)?;
    validate_x402_payment_terms(&challenge, 0, 1_001)?;
    assert!(matches!(
        admit_payment_requirement(&challenge, 0, 1_001),
        Err(X402PolicyError::InsecureOrigin)
    ));
    assert_eq!(
        validate_x402_payment_terms(&challenge, 0, 1_060),
        Err(X402PolicyError::ExpiredRequirement)
    );
    assert_eq!(
        validate_x402_payment_terms(&challenge, 1, 1_001),
        Err(X402PolicyError::UnknownRequirement)
    );
    value["accepts"][0]["network"] = json!("eip155:1");
    assert_eq!(
        validate_x402_payment_terms(&parse(&value, local_url)?, 0, 1_001),
        Err(X402PolicyError::UnsupportedNetwork)
    );
    Ok(())
}

#[cfg(debug_assertions)]
#[test]
fn debug_loopback_admission_is_scoped_and_distinct_from_production() -> Result<(), Box<dyn Error>> {
    let local_url = "http://127.0.0.1:5001/test-project/us-central1/api/x402/resource/run-1";
    let expected_origin = "http://127.0.0.1:5001";
    let mut value = valid_challenge();
    value["resource"]["url"] = json!(local_url);
    let challenge = parse(&value, local_url)?;
    let admitted = admit_debug_loopback_payment_requirement(&challenge, 0, 1_001, expected_origin)?;
    assert_eq!(admitted.origin_policy(), X402OriginPolicy::DebugLoopback);
    assert!(matches!(
        admit_payment_requirement(&challenge, 0, 1_001),
        Err(X402PolicyError::InsecureOrigin)
    ));

    let alias = local_url.replace("127.0.0.1", "localhost");
    value["resource"]["url"] = json!(alias);
    let challenge = parse(&value, &alias)?;
    assert!(matches!(
        admit_debug_loopback_payment_requirement(&challenge, 0, 1_001, expected_origin),
        Err(X402PolicyError::DisallowedDomain)
    ));
    Ok(())
}

#[cfg(debug_assertions)]
#[test]
fn debug_loopback_admission_accepts_caller_bound_routes_on_the_expected_origin()
-> Result<(), Box<dyn Error>> {
    let purchase = "http://127.0.0.1:5001/test-project/us-central1/api/x402/offers/abc/purchase";
    let expected_origin = "http://127.0.0.1:5001";
    let mut value = valid_challenge();
    value["resource"]["url"] = json!(purchase);
    assert!(
        admit_debug_loopback_payment_requirement(
            &parse(&value, purchase)?,
            0,
            1_001,
            expected_origin
        )
        .is_ok()
    );

    for rejected in [
        "http://127.0.0.1:5002/test-project/us-central1/api/x402/offers/abc/purchase",
        "http://localhost:5001/test-project/us-central1/api/x402/offers/abc/purchase",
        "https://127.0.0.1:5001/test-project/us-central1/api/x402/offers/abc/purchase",
    ] {
        value["resource"]["url"] = json!(rejected);
        let challenge = parse(&value, rejected)?;
        assert!(matches!(
            admit_debug_loopback_payment_requirement(&challenge, 0, 1_001, expected_origin),
            Err(X402PolicyError::DisallowedDomain)
        ));
    }
    Ok(())
}

#[test]
fn explicit_eip3009_authorization_mode_is_supported() -> Result<(), Box<dyn Error>> {
    let mut value = valid_challenge();
    value["accepts"][0]["extra"]["assetTransferMethod"] = json!("eip3009");
    value["accepts"][0]["extra"]["paymentFlow"] = json!("authorization");
    let challenge = parse(&value, RESOURCE_URL)?;
    assert!(admit_payment_requirement(&challenge, 0, 1_001).is_ok());
    Ok(())
}

#[test]
fn origin_policy_requires_canonical_domain_https_on_default_port() -> Result<(), Box<dyn Error>> {
    for (url, expected) in [
        (
            "http://pay.flowzed.com/premium-data?market=eth",
            X402PolicyError::InsecureOrigin,
        ),
        (
            "https://pay.flowzed.com:8443/premium-data?market=eth",
            X402PolicyError::DisallowedPort,
        ),
        (
            "https://127.0.0.1/premium-data?market=eth",
            X402PolicyError::DisallowedDomain,
        ),
        (
            "https://localhost/premium-data?market=eth",
            X402PolicyError::DisallowedDomain,
        ),
        (
            "https://service.local/premium-data?market=eth",
            X402PolicyError::DisallowedDomain,
        ),
    ] {
        let mut value = valid_challenge();
        value["resource"]["url"] = json!(url);
        assert_eq!(policy_error(&value, url)?, expected, "{url}");
    }

    let normalized = "HTTPS://PAY.FLOWZED.COM:443/premium-data?market=eth";
    let mut value = valid_challenge();
    value["resource"]["url"] = json!(normalized);
    let challenge = parse(&value, normalized)?;
    let admitted = admit_payment_requirement(&challenge, 0, 1_001)?;
    assert_eq!(admitted.resource().origin(), "https://pay.flowzed.com");
    Ok(())
}

#[test]
fn unrelated_networks_are_not_admitted() -> Result<(), Box<dyn Error>> {
    for network in ["eip155:10", "eip155:1", "eip155:11155111"] {
        let mut value = valid_challenge();
        value["accepts"][0]["network"] = json!(network);
        assert_eq!(
            policy_error(&value, RESOURCE_URL)?,
            X402PolicyError::UnsupportedNetwork,
            "{network}"
        );
    }
    Ok(())
}

#[test]
fn only_exact_and_chain_bound_usdc_are_admitted() -> Result<(), Box<dyn Error>> {
    let mut value = valid_challenge();
    value["accepts"][0]["scheme"] = json!("upto");
    assert_eq!(
        policy_error(&value, RESOURCE_URL)?,
        X402PolicyError::UnsupportedScheme
    );

    value = valid_challenge();
    value["accepts"][0]["asset"] = json!("0x0000000000000000000000000000000000000001");
    assert_eq!(
        policy_error(&value, RESOURCE_URL)?,
        X402PolicyError::UnsupportedAsset
    );

    value = valid_challenge();
    value["accepts"][0]["asset"] = json!("0x036CbD53842c5426634e7929541eC2318f3dCF7e");
    value["accepts"][0]["payTo"] = json!("0x209693Bc6afc0C5328bA36FaF03C514EF312287C");
    let challenge = parse(&value, RESOURCE_URL)?;
    let admitted = admit_payment_requirement(&challenge, 0, 1_001)?;
    assert_eq!(admitted.asset().token_contract(), Some(ASSET));
    assert_eq!(admitted.recipient(), RECIPIENT);
    Ok(())
}

#[test]
fn extensions_transfer_methods_and_payment_flows_fail_closed() -> Result<(), Box<dyn Error>> {
    let mut value = valid_challenge();
    value["extensions"] = json!({"unknown": {"info": {}, "schema": {}}});
    assert_eq!(
        policy_error(&value, RESOURCE_URL)?,
        X402PolicyError::UnsupportedExtension
    );

    value = valid_challenge();
    value["accepts"][0]["extra"]["assetTransferMethod"] = json!("permit2");
    assert_eq!(
        policy_error(&value, RESOURCE_URL)?,
        X402PolicyError::UnsupportedAssetTransferMethod
    );

    value = valid_challenge();
    value["accepts"][0]["extra"]["paymentFlow"] = json!("upfront");
    assert_eq!(
        policy_error(&value, RESOURCE_URL)?,
        X402PolicyError::UnsupportedPaymentFlow
    );

    value = valid_challenge();
    value["accepts"][0]["extra"]["facilitator"] = json!("https://evil.example");
    assert_eq!(
        policy_error(&value, RESOURCE_URL)?,
        X402PolicyError::InvalidSchemeData
    );
    Ok(())
}

#[test]
fn bazaar_cannot_override_payment_terms_or_origin_policy() -> Result<(), Box<dyn Error>> {
    let mut value = valid_challenge();
    value["extensions"] = json!({"bazaar": {
        "info": {"input": {"type": "http", "method": "GET"},
                 "network": "eip155:84532", "asset": ASSET, "paymentFlow": "authorization"},
        "schema": {"type": "object"}
    }});
    let challenge = parse(&value, RESOURCE_URL)?;
    assert!(admit_payment_requirement(&challenge, 0, 1_001).is_ok());
    assert_eq!(
        admit_payment_requirement(&challenge, 0, 1_060).err(),
        Some(X402PolicyError::ExpiredRequirement)
    );
    for (pointer, replacement, expected) in [
        (
            "/accepts/0/network",
            "eip155:1",
            X402PolicyError::UnsupportedNetwork,
        ),
        (
            "/accepts/0/asset",
            RECIPIENT,
            X402PolicyError::UnsupportedAsset,
        ),
        (
            "/accepts/0/scheme",
            "upto",
            X402PolicyError::UnsupportedScheme,
        ),
        (
            "/accepts/0/extra/assetTransferMethod",
            "permit2",
            X402PolicyError::UnsupportedAssetTransferMethod,
        ),
        (
            "/accepts/0/extra/paymentFlow",
            "upfront",
            X402PolicyError::UnsupportedPaymentFlow,
        ),
    ] {
        let mut changed = value.clone();
        changed["accepts"][0]["extra"]["assetTransferMethod"] = json!("eip3009");
        changed["accepts"][0]["extra"]["paymentFlow"] = json!("authorization");
        *changed.pointer_mut(pointer).ok_or("missing test field")? = json!(replacement);
        assert_eq!(policy_error(&changed, RESOURCE_URL)?, expected);
    }
    let local_url = "http://127.0.0.1:5001/purchase";
    value["resource"]["url"] = json!(local_url);
    assert_eq!(
        policy_error(&value, local_url)?,
        X402PolicyError::InsecureOrigin
    );
    #[cfg(debug_assertions)]
    {
        let challenge = parse(&value, local_url)?;
        assert!(
            admit_debug_loopback_payment_requirement(&challenge, 0, 1_001, "http://127.0.0.1:5001")
                .is_ok()
        );
        assert_eq!(
            admit_debug_loopback_payment_requirement(&challenge, 0, 1_001, "http://127.0.0.1:5002")
                .err(),
            Some(X402PolicyError::DisallowedDomain)
        );
    }
    Ok(())
}

#[test]
fn remote_asset_metadata_cannot_redefine_trusted_usdc() -> Result<(), Box<dyn Error>> {
    for (field, replacement) in [("name", json!("USD Coin")), ("version", json!("1"))] {
        let mut value = valid_challenge();
        value["accepts"][0]["extra"][field] = replacement;
        assert_eq!(
            policy_error(&value, RESOURCE_URL)?,
            X402PolicyError::InvalidSchemeData,
            "{field}"
        );
    }

    let mut missing = valid_challenge();
    missing["accepts"][0]
        .as_object_mut()
        .ok_or("missing requirement")?
        .remove("extra");
    assert_eq!(
        policy_error(&missing, RESOURCE_URL)?,
        X402PolicyError::InvalidSchemeData
    );
    Ok(())
}

#[test]
fn expiry_and_requirement_identity_are_rechecked() -> Result<(), Box<dyn Error>> {
    let challenge = parse(&valid_challenge(), RESOURCE_URL)?;
    assert_eq!(
        admit_payment_requirement(&challenge, 0, 1_060).err(),
        Some(X402PolicyError::ExpiredRequirement)
    );
    assert_eq!(
        admit_payment_requirement(&challenge, 1, 1_001).err(),
        Some(X402PolicyError::UnknownRequirement)
    );
    Ok(())
}

#[cfg(debug_assertions)]
#[test]
fn loopback_qualification_never_admits_valid_mainnet_terms() -> Result<(), Box<dyn Error>> {
    let url = "http://127.0.0.1:5001/qualification/resource";
    let mut value = valid_challenge();
    value["resource"]["url"] = json!(url);
    value["accepts"][0]["network"] = json!("eip155:8453");
    value["accepts"][0]["asset"] = json!("0x833589fcd6edb6e08f4c7c32d4f71b54bda02913");
    value["accepts"][0]["extra"]["name"] = json!("USD Coin");
    let challenge = parse(&value, url)?;
    assert_eq!(
        admit_debug_loopback_payment_requirement(&challenge, 0, 1001, "http://127.0.0.1:5001")
            .err(),
        Some(X402PolicyError::UnsupportedNetwork)
    );
    Ok(())
}
