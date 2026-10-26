//! Debug-only scoped loopback transport for x402 interoperability testing.

mod deadlines;

use deadlines::ResponseDeadlines;

use std::time::Duration;

use reqwest::{
    Client, Method,
    header::{ACCEPT, ACCEPT_ENCODING, AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, HeaderValue},
};
use sha2::{Digest as _, Sha256};
use tokio::time::timeout;
use url::{Host, Url};

use crate::{
    WebBrokerError, WebCancellation, WebMethod, WebPaymentSignature,
    response::{
        BodyAccumulator, BodyDisposition, ValidatedResponseMetadata, validate_response_metadata,
    },
};

const REQUEST_IDENTITY_DOMAIN: &[u8] = b"zeta.debug-x402-request.v1";
const MAX_REQUEST_BODY_BYTES: usize = 32 * 1024;
const MAX_BODY_BYTES: usize = 64 * 1024;
const BODY_IDLE_TIMEOUT: Duration = Duration::from_secs(5);

/// Closed methods supported by the local x402 interoperability transport.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DebugX402Method {
    /// Retrieve one exact local representation.
    Get,
    /// Submit one exact local JSON request or empty paid request.
    Post,
}

impl DebugX402Method {
    /// Returns the exact uppercase HTTP spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
        }
    }
}

/// Exact local request retained across challenge, approval and one paid retry.
///
/// The bearer capability is deliberately not stored here. Its digest is bound
/// into `identity_sha256`, while the owning desktop workflow retains the actual
/// value outside model-visible and persisted tool state.
#[derive(Clone, Eq, PartialEq)]
pub struct DebugX402Request {
    method: DebugX402Method,
    url: String,
    body: Vec<u8>,
}

impl DebugX402Request {
    /// Validates a request inside one configured debug loopback scope.
    ///
    /// # Errors
    ///
    /// Returns a closed client error for a disallowed route, method, body size
    /// or JSON representation.
    pub fn new(
        scope: &DebugX402Scope,
        method: DebugX402Method,
        url: &str,
        body: Vec<u8>,
    ) -> Result<Self, WebBrokerError> {
        if !scope.admits(url)
            || body.len() > MAX_REQUEST_BODY_BYTES
            || (method == DebugX402Method::Get && !body.is_empty())
        {
            return Err(WebBrokerError::ClientUnavailable);
        }
        if !body.is_empty()
            && serde_json::from_slice::<serde_json::Value>(&body)
                .ok()
                .is_none_or(|value| !value.is_object())
        {
            return Err(WebBrokerError::ClientUnavailable);
        }
        Ok(Self {
            method,
            url: url.to_owned(),
            body,
        })
    }

    #[must_use]
    /// Returns the bound HTTP method.
    pub const fn method(&self) -> DebugX402Method {
        self.method
    }

    #[must_use]
    /// Returns the exact canonical URL.
    pub fn url(&self) -> &str {
        &self.url
    }

    #[must_use]
    /// Returns the exact request body bytes.
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    #[must_use]
    /// Returns the host-owned identity of method, URL, body, bearer and transport policy.
    pub fn identity_sha256(&self, bearer: Option<&str>) -> [u8; 32] {
        request_identity_sha256(self.method, &self.url, &self.body, bearer)
    }
}

fn request_identity_sha256(
    method: DebugX402Method,
    url: &str,
    body: &[u8],
    bearer: Option<&str>,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(REQUEST_IDENTITY_DOMAIN);
    hash_field(&mut hasher, method.as_str().as_bytes());
    hash_field(&mut hasher, url.as_bytes());
    hash_field(&mut hasher, &Sha256::digest(body));
    hash_field(
        &mut hasher,
        &bearer.map_or_else(
            || Sha256::digest([]),
            |value| Sha256::digest(value.as_bytes()),
        ),
    );
    hash_field(&mut hasher, b"no-proxy/no-redirect/no-retry/http1/identity");
    hasher.finalize().into()
}

/// Machine-local URL subtree allowed for debug x402 interoperability.
#[derive(Clone, Eq, PartialEq)]
pub struct DebugX402Scope {
    origin: String,
    path_prefix: String,
}

impl DebugX402Scope {
    /// Parses an HTTP loopback base URL with a non-root path and explicit port.
    ///
    /// # Errors
    ///
    /// Returns a closed client error for public hosts, aliases, credentials,
    /// queries, fragments, default ports or ambiguous paths.
    pub fn parse(value: &str) -> Result<Self, WebBrokerError> {
        let url = Url::parse(value).map_err(|_| WebBrokerError::ClientUnavailable)?;
        if url.scheme() != "http"
            || url.username() != ""
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !matches!(url.host(), Some(Host::Ipv4(address)) if address.is_loopback())
            || url.port().is_none()
            || url.path() == "/"
            || value.ends_with('/')
            || url.as_str() != value
        {
            return Err(WebBrokerError::ClientUnavailable);
        }
        Ok(Self {
            origin: url.origin().ascii_serialization(),
            path_prefix: url.path().to_owned(),
        })
    }

    /// Returns whether an exact URL is within this configured origin and path subtree.
    #[must_use]
    pub fn admits(&self, value: &str) -> bool {
        let Ok(url) = Url::parse(value) else {
            return false;
        };
        let path = url.path();
        url.username() == ""
            && url.password().is_none()
            && url.fragment().is_none()
            && url.as_str() == value
            && url.origin().ascii_serialization() == self.origin
            && (path == self.path_prefix
                || path
                    .strip_prefix(&self.path_prefix)
                    .is_some_and(|suffix| suffix.starts_with('/')))
    }

    /// Returns the configured canonical loopback origin.
    #[must_use]
    pub fn origin(&self) -> &str {
        &self.origin
    }
}

/// Bounded response facts retained for strict x402 handling.
pub struct DebugX402Response {
    status: u16,
    body: Vec<u8>,
    payment_required_headers: Vec<Vec<u8>>,
    payment_response_headers: Vec<Vec<u8>>,
}

impl DebugX402Response {
    #[must_use]
    /// Returns the numeric HTTP status.
    pub const fn status(&self) -> u16 {
        self.status
    }
    #[must_use]
    /// Returns bounded untrusted response content.
    pub fn body(&self) -> &[u8] {
        &self.body
    }
    #[must_use]
    /// Returns all retained payment-required header values.
    pub fn payment_required_headers(&self) -> &[Vec<u8>] {
        &self.payment_required_headers
    }
    #[must_use]
    /// Returns all retained payment-response header values.
    pub fn payment_response_headers(&self) -> &[Vec<u8>] {
        &self.payment_response_headers
    }
}

/// Debug-only no-proxy, no-redirect, no-retry loopback broker.
#[derive(Clone, Copy, Default)]
pub struct DebugX402Broker;

impl DebugX402Broker {
    /// Executes one exact local discovery, composition or protected request.
    ///
    /// # Errors
    ///
    /// Returns a bounded broker error for cancellation, timeout, invalid
    /// response policy, body limits or transport failure.
    pub async fn execute_request(
        &self,
        request: &DebugX402Request,
        bearer: Option<&str>,
        payment: Option<&WebPaymentSignature>,
        cancellation: &WebCancellation,
    ) -> Result<DebugX402Response, WebBrokerError> {
        self.execute_with_deadlines(
            request,
            bearer,
            payment,
            cancellation,
            ResponseDeadlines::for_request(payment.is_some()),
        )
        .await
    }

    async fn execute_with_deadlines(
        &self,
        request: &DebugX402Request,
        bearer: Option<&str>,
        payment: Option<&WebPaymentSignature>,
        cancellation: &WebCancellation,
        deadlines: ResponseDeadlines,
    ) -> Result<DebugX402Response, WebBrokerError> {
        self.execute_with_admission_and_deadlines(
            request,
            bearer,
            payment,
            cancellation,
            deadlines,
            None,
        )
        .await
    }

    /// Executes once, asking the host for final admission immediately before the send.
    /// # Errors
    /// Returns `SubmissionRejected` with no send if lifecycle or exact binding changed.
    pub async fn execute_request_with_admission(
        &self,
        request: &DebugX402Request,
        bearer: Option<&str>,
        payment: &WebPaymentSignature,
        cancellation: &WebCancellation,
        admit: impl FnOnce() -> bool + Send + 'static,
    ) -> Result<DebugX402Response, WebBrokerError> {
        self.execute_with_admission_and_deadlines(
            request,
            bearer,
            Some(payment),
            cancellation,
            ResponseDeadlines::for_request(true),
            Some(Box::new(admit)),
        )
        .await
    }

    #[expect(
        clippy::too_many_lines,
        reason = "Exact request construction, final admission and bounded response handling remain visible as one no-retry boundary"
    )]
    async fn execute_with_admission_and_deadlines(
        &self,
        request: &DebugX402Request,
        bearer: Option<&str>,
        payment: Option<&WebPaymentSignature>,
        cancellation: &WebCancellation,
        deadlines: ResponseDeadlines,
        admit: Option<Box<dyn FnOnce() -> bool + Send>>,
    ) -> Result<DebugX402Response, WebBrokerError> {
        if cancellation.is_cancelled() {
            return Err(WebBrokerError::Cancelled);
        }
        let operation = async {
            let client = Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .http1_only()
                .connect_timeout(Duration::from_secs(3))
                // Response deadlines are enforced around the futures below.
                // A client-wide read timeout also limits header waiting, and
                // a duplicate total timer can misclassify a total deadline as
                // a header timeout. Keep one owner for each response phase.
                .pool_max_idle_per_host(0)
                .build()
                .map_err(|_| WebBrokerError::ClientUnavailable)?;
            let method = match request.method() {
                DebugX402Method::Get => Method::GET,
                DebugX402Method::Post => Method::POST,
            };
            let mut outbound = client
                .request(method, request.url())
                .header(
                    ACCEPT,
                    HeaderValue::from_static("application/json, text/plain"),
                )
                .header(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
            if !request.body().is_empty() {
                outbound = outbound
                    .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
                    .body(request.body().to_vec());
            }
            if let Some(bearer) = bearer {
                let value = HeaderValue::from_str(&format!("Bearer {bearer}"))
                    .map_err(|_| WebBrokerError::ClientUnavailable)?;
                outbound = outbound.header(AUTHORIZATION, value);
            }
            if let Some(payment) = payment {
                outbound = outbound.header("payment-signature", payment.as_str());
            }
            let outbound = outbound
                .build()
                .map_err(|_| WebBrokerError::ClientUnavailable)?;
            if outbound.url().as_str() != request.url()
                || outbound.method().as_str() != request.method().as_str()
            {
                return Err(WebBrokerError::ClientUnavailable);
            }
            if let Some(admit) = admit
                && !admit()
            {
                return Err(WebBrokerError::SubmissionRejected);
            }
            let mut response = tokio::select! {
                () = cancellation.cancelled() => return Err(WebBrokerError::Cancelled),
                result = timeout(deadlines.headers, client.execute(outbound)) => match result {
                    Err(_) => return Err(WebBrokerError::ConnectionOrHeaderDeadline),
                    Ok(Err(error)) if error.is_timeout() => return Err(WebBrokerError::ConnectionOrHeaderDeadline),
                    Ok(Err(_)) => return Err(WebBrokerError::TransportFailed),
                    Ok(Ok(response)) => response,
                }
            };
            validate_headers(&response)?;
            let ValidatedResponseMetadata {
                status,
                disposition,
                declared_length,
                max_body_bytes,
                ..
            } = validate_response_metadata(
                WebMethod::Get,
                response.status(),
                16,
                response.headers(),
                MAX_BODY_BYTES,
                MAX_BODY_BYTES,
            )
            .map_err(WebBrokerError::ResponseRejected)?;
            let payment_required_headers = if status == 402 {
                copy_headers(&response, "payment-required")
            } else {
                Vec::new()
            };
            let payment_response_headers = copy_headers(&response, "payment-response");
            let media_type = match disposition {
                BodyDisposition::Accept(media_type) => media_type,
                BodyDisposition::Discard(_) => {
                    return Ok(DebugX402Response {
                        status,
                        body: Vec::new(),
                        payment_required_headers,
                        payment_response_headers,
                    });
                }
            };
            let mut body =
                BodyAccumulator::new(media_type, declared_length, max_body_bytes, MAX_BODY_BYTES);
            loop {
                let chunk = tokio::select! {
                    () = cancellation.cancelled() => return Err(WebBrokerError::Cancelled),
                    result = timeout(BODY_IDLE_TIMEOUT, response.chunk()) => match result {
                        Err(_) => return Err(WebBrokerError::BodyIdleDeadline),
                        Ok(Err(_)) => return Err(WebBrokerError::TransportFailed),
                        Ok(Ok(chunk)) => chunk,
                    }
                };
                let Some(chunk) = chunk else { break };
                body.push_identity_chunk(&chunk)
                    .map_err(WebBrokerError::ResponseRejected)?;
            }
            let body = body.finish().map_err(WebBrokerError::ResponseRejected)?;
            Ok(DebugX402Response {
                status,
                body,
                payment_required_headers,
                payment_response_headers,
            })
        };
        timeout(deadlines.total, operation)
            .await
            .unwrap_or(Err(WebBrokerError::TotalDeadline))
    }
}

fn validate_headers(response: &reqwest::Response) -> Result<(), WebBrokerError> {
    let headers = response.headers();
    if headers.len() > 64
        || headers
            .iter()
            .any(|(name, value)| name.as_str().len().saturating_add(value.len()) > 8 * 1024)
        || headers
            .iter()
            .map(|(name, value)| name.as_str().len().saturating_add(value.len()))
            .sum::<usize>()
            > 32 * 1024
    {
        return Err(WebBrokerError::ResponseRejected(
            crate::ResponseValidationError::HeaderAggregateLimit,
        ));
    }
    let no_store = headers.get_all(CACHE_CONTROL).iter().any(|value| {
        value.to_str().is_ok_and(|value| {
            value
                .split(',')
                .any(|part| part.trim().eq_ignore_ascii_case("no-store"))
        })
    });
    if !no_store {
        return Err(WebBrokerError::ResponseRejected(
            crate::ResponseValidationError::UnsupportedEncoding,
        ));
    }
    Ok(())
}

fn copy_headers(response: &reqwest::Response, name: &'static str) -> Vec<Vec<u8>> {
    response
        .headers()
        .get_all(name)
        .iter()
        .map(|value| value.as_bytes().to_vec())
        .collect()
}

fn hash_field(hasher: &mut Sha256, value: &[u8]) {
    hasher.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    hasher.update(value);
}

#[cfg(test)]
#[path = "debug_x402/transport_tests.rs"]
mod transport_tests;

#[cfg(test)]
mod tests {
    use super::*;

    const BASE_URL: &str = "http://127.0.0.1:5001/test-project/us-central1/api/x402";

    #[test]
    fn scope_accepts_only_its_exact_loopback_subtree() -> Result<(), WebBrokerError> {
        let scope = DebugX402Scope::parse(BASE_URL)?;
        assert!(scope.admits(BASE_URL));
        assert!(scope.admits(&format!("{BASE_URL}/atelier/quotes?currency=usdc")));
        assert!(!scope.admits("http://127.0.0.1:5001/control/runs"));
        assert!(!scope.admits(&format!("{BASE_URL}-alias/resource")));
        assert!(!scope.admits(&BASE_URL.replace("127.0.0.1", "localhost")));
        assert!(!scope.admits(&BASE_URL.replace(":5001", ":5002")));
        Ok(())
    }

    #[test]
    fn scope_rejects_ambiguous_or_public_base_urls() {
        assert!(DebugX402Scope::parse("http://127.0.0.1:5001/").is_err());
        assert!(DebugX402Scope::parse("http://localhost:5001/api/x402").is_err());
        assert!(DebugX402Scope::parse("http://192.0.2.1:5001/api/x402").is_err());
        assert!(DebugX402Scope::parse("https://127.0.0.1:5001/api/x402").is_err());
        assert!(DebugX402Scope::parse("http://127.0.0.1/api/x402").is_err());
        assert!(DebugX402Scope::parse("http://127.0.0.1:5001/api/x402?scope=wide").is_err());
    }

    #[test]
    fn request_identity_binds_method_body_and_private_bearer_digest() -> Result<(), WebBrokerError>
    {
        let scope = DebugX402Scope::parse(BASE_URL)?;
        let url = format!("{BASE_URL}/atelier/quotes");
        let get = DebugX402Request::new(&scope, DebugX402Method::Get, &url, Vec::new())?;
        let post = DebugX402Request::new(
            &scope,
            DebugX402Method::Post,
            &url,
            br#"{"quote":{}}"#.to_vec(),
        )?;
        assert_ne!(get.identity_sha256(None), post.identity_sha256(None));
        assert_ne!(
            post.identity_sha256(Some("a")),
            post.identity_sha256(Some("b"))
        );
        assert_ne!(post.identity_sha256(None), post.identity_sha256(Some("a")));
        Ok(())
    }
}
