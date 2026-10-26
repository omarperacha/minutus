use std::{
    error::Error,
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};

use super::{
    inspect::{Expectation, inspect},
    transport::{Observation, RESOURCE_PREFIX, ResourceUrl, client, request},
};

fn resource() -> Result<ResourceUrl, &'static str> {
    ResourceUrl::parse(&format!("{RESOURCE_PREFIX}run-1"))
}

fn fixture() -> Value {
    json!({
        "x402Version": 2,
        "resource": {"url": format!("{RESOURCE_PREFIX}run-1")},
        "accepts": [{
            "scheme": "exact", "network": "eip155:84532", "amount": "10000",
            "asset": "0x036cbd53842c5426634e7929541ec2318f3dcf7e",
            "payTo": "0x3Cb0d132c7752E6ddbEB4a2a1Bb9B43DA1d90DF5",
            "maxTimeoutSeconds": 60,
            "extra": {"name": "USDC", "version": "2", "assetTransferMethod": "eip3009", "paymentFlow": "authorization"}
        }]
    })
}

fn observation(value: &Value) -> Result<Observation, serde_json::Error> {
    Ok(Observation {
        status: 402,
        payment_required: vec![STANDARD.encode(serde_json::to_vec(value)?).into_bytes()],
    })
}

#[test]
fn only_exact_loopback_resource_route_is_accepted() -> Result<(), Box<dyn Error>> {
    resource()?;
    let url = format!("{RESOURCE_PREFIX}run-1");
    for forbidden in [
        url.replace("127.0.0.1", "localhost"),
        url.replace("127.0.0.1", "127.1"),
        url.replace("127.0.0.1", "2130706433"),
        url.replace("127.0.0.1", "[::1]"),
        url.replace("127.0.0.1", "example.com"),
        url.replace(":5001", ":5002"),
        url.replace("http:", "https:"),
        url.replace("/resources/", "/control/runs/"),
        url.replace("/resources/", "/atelier/quotes/"),
        format!("{url}?secret=hidden"),
        format!("{url}#fragment"),
        format!("{RESOURCE_PREFIX}../control"),
        format!("{RESOURCE_PREFIX}%72un-1"),
        format!("{url}/"),
        format!("{url}\n"),
        RESOURCE_PREFIX.to_owned(),
        format!("{RESOURCE_PREFIX}{}", "a".repeat(129)),
    ] {
        assert!(ResourceUrl::parse(&forbidden).is_err());
    }
    Ok(())
}

#[test]
fn request_is_exact_unpaid_get_without_credentials_or_body() -> Result<(), Box<dyn Error>> {
    let resource = resource()?;
    let request = request(&client()?, &resource)?;
    assert_eq!(request.method(), reqwest::Method::GET);
    assert_eq!(request.url().as_str(), resource.as_str());
    assert!(request.body().is_none());
    assert_eq!(request.headers().len(), 2);
    assert!(!request.headers().contains_key("payment-signature"));
    assert!(!request.headers().contains_key("authorization"));
    assert!(!request.headers().contains_key("cookie"));
    Ok(())
}

#[test]
fn real_parser_accepts_backend_terms_without_admitting_payment() -> Result<(), Box<dyn Error>> {
    assert!(
        inspect(
            &resource()?,
            &observation(&fixture())?,
            Expectation::Valid,
            1_000
        )?
        .contains("production origin policy remains closed")
    );
    Ok(())
}

#[test]
fn every_security_term_and_resource_substitution_fails_preflight() -> Result<(), Box<dyn Error>> {
    for (pointer, replacement) in [
        ("/x402Version", json!(1)),
        (
            "/resource/url",
            json!(format!("{RESOURCE_PREFIX}other-run")),
        ),
        ("/accepts/0/scheme", json!("upto")),
        ("/accepts/0/network", json!("eip155:8453")),
        (
            "/accepts/0/asset",
            json!("0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"),
        ),
        ("/accepts/0/amount", json!("20000")),
        ("/accepts/0/payTo", json!("not-an-address")),
        ("/accepts/0/maxTimeoutSeconds", json!(0)),
        ("/accepts/0/extra/name", json!("USD Coin")),
        ("/accepts/0/extra/assetTransferMethod", json!("permit2")),
        ("/accepts/0/extra/paymentFlow", json!("deferred")),
    ] {
        let mut value = fixture();
        *value.pointer_mut(pointer).ok_or("missing fixture field")? = replacement;
        assert!(
            inspect(
                &resource()?,
                &observation(&value)?,
                Expectation::Valid,
                1_000
            )
            .is_err(),
            "{pointer}"
        );
    }
    let mut value = fixture();
    value["extensions"] = json!({"unsupported": {}});
    assert!(
        inspect(
            &resource()?,
            &observation(&value)?,
            Expectation::Valid,
            1_000
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn short_quote_checks_local_expiry_without_a_wallet_or_second_request() -> Result<(), Box<dyn Error>>
{
    let mut value = fixture();
    value["accepts"][0]["maxTimeoutSeconds"] = json!(1);
    let observation = observation(&value)?;
    assert!(inspect(&resource()?, &observation, Expectation::ShortLived, 1_000).is_ok());
    assert!(inspect(&resource()?, &observation, Expectation::Valid, 1_000).is_err());
    Ok(())
}

#[test]
fn malformed_expectation_does_not_mask_missing_or_duplicate_headers() -> Result<(), Box<dyn Error>>
{
    let resource = resource()?;
    let mut observation = Observation {
        status: 402,
        payment_required: vec![b"!invalid-base64!".to_vec()],
    };
    assert!(inspect(&resource, &observation, Expectation::Malformed, 1_000).is_ok());
    assert!(inspect(&resource, &observation, Expectation::Valid, 1_000).is_err());
    observation.payment_required.clear();
    assert!(inspect(&resource, &observation, Expectation::Malformed, 1_000).is_err());
    observation.payment_required = vec![b"bad".to_vec(), b"bad".to_vec()];
    assert!(inspect(&resource, &observation, Expectation::Malformed, 1_000).is_err());
    observation.status = 503;
    assert!(inspect(&resource, &observation, Expectation::Malformed, 1_000).is_err());
    Ok(())
}

#[test]
fn redirect_client_never_contacts_the_location_target() -> Result<(), Box<dyn Error>> {
    let source = TcpListener::bind("127.0.0.1:0")?;
    let destination = TcpListener::bind("127.0.0.1:0")?;
    destination.set_nonblocking(true)?;
    let address = source.local_addr()?;
    let destination_address = destination.local_addr()?;
    let server = thread::spawn(move || -> std::io::Result<()> {
        let (mut stream, _) = source.accept()?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        let mut bytes = [0; 4096];
        if stream.read(&mut bytes)? == 0 {
            return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
        }
        write!(
            stream,
            "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{destination_address}/must-not-fetch\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )?;
        Ok(())
    });
    // Test-only ephemeral listener; the runnable probe accepts only RESOURCE_PREFIX.
    let response = client()?.get(format!("http://{address}/")).send()?;
    server.join().map_err(|_| "fixture server failed")??;
    assert_eq!(response.status().as_u16(), 307);
    assert!(
        matches!(destination.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );
    Ok(())
}
