use std::time::Duration;

use reqwest::{
    blocking::{Client, Request},
    header::{ACCEPT, ACCEPT_ENCODING, CACHE_CONTROL},
};

/// The exact operator-supplied diagnostic endpoint; cannot name control routes.
pub(super) struct ResourceUrl(String);

pub(super) const RESOURCE_PREFIX: &str =
    "http://127.0.0.1:5001/glass-handler-414009/us-central1/api/api/internal/x402/resources/";

impl ResourceUrl {
    pub(super) fn parse(value: &str) -> Result<Self, &'static str> {
        let id = value
            .strip_prefix(RESOURCE_PREFIX)
            .ok_or("only the configured loopback diagnostic resource route is permitted")?;
        if id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(
                "invalid diagnostic run ID; aliases, queries and URL normalization are forbidden",
            );
        }
        Ok(Self(value.to_owned()))
    }

    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
}

pub(super) struct Observation {
    pub(super) status: u16,
    pub(super) payment_required: Vec<Vec<u8>>,
}

pub(super) fn client() -> Result<Client, &'static str> {
    Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .http1_only()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|_| "local HTTP client unavailable")
}

pub(super) fn request(client: &Client, resource: &ResourceUrl) -> Result<Request, &'static str> {
    let request = client
        .get(resource.as_str())
        .header(ACCEPT, "application/json")
        .header(ACCEPT_ENCODING, "identity")
        .build()
        .map_err(|_| "invalid local HTTP request")?;
    if request.url().as_str() != resource.as_str() {
        return Err("HTTP client changed the exact resource URL");
    }
    Ok(request)
}

pub(super) fn fetch(resource: &ResourceUrl) -> Result<Observation, &'static str> {
    let client = client()?;
    let response = client
        .execute(request(&client, resource)?)
        .map_err(|_| "local emulator unavailable or request timed out; no retry was attempted")?;
    let headers = response.headers();
    if headers.len() > 64
        || headers
            .iter()
            .any(|(name, value)| name.as_str().len() + value.len() > 8 * 1024)
        || headers
            .iter()
            .map(|(name, value)| name.as_str().len() + value.len())
            .sum::<usize>()
            > 32 * 1024
    {
        return Err("response headers exceed the bounded transport contract");
    }
    let no_store = headers.get_all(CACHE_CONTROL).iter().any(|value| {
        value.to_str().is_ok_and(|value| {
            value
                .split(',')
                .any(|part| part.trim().eq_ignore_ascii_case("no-store"))
        })
    });
    if !no_store {
        return Err("emulator response must specify Cache-Control: no-store");
    }
    // Never read bodies, Location, cookies, descriptions or remote error prose.
    Ok(Observation {
        status: response.status().as_u16(),
        payment_required: headers
            .get_all("payment-required")
            .iter()
            .map(|value| value.as_bytes().to_vec())
            .collect(),
    })
}
