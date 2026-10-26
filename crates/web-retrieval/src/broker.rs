use std::{
    fmt,
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use reqwest::{
    Client, Method,
    header::{
        ACCEPT, ACCEPT_ENCODING, AUTHORIZATION, COOKIE, HeaderMap, HeaderValue, ORIGIN,
        PROXY_AUTHORIZATION, REFERER, USER_AGENT,
    },
    redirect,
};
use tokio::{sync::Notify, time::timeout};

use crate::{
    HostResolver, MAX_DECODED_BODY_BYTES, MAX_ENCODED_BODY_BYTES,
    MAX_REMOTE_ASSET_DECODED_BODY_BYTES, MAX_REMOTE_ASSET_ENCODED_BODY_BYTES,
    ResponseValidationError, TargetValidationError, ValidatedConnectionPlan, ValidatedWebMediaType,
    WebMethod, WebPaymentRequiredChallenge, WebRequest, WebResponseKind, WebResponseProvenance,
    response::{
        BodyAccumulator, BodyDisposition, ValidatedResponseMetadata, validate_response_metadata,
    },
};

/// DNS phase deadline.
pub const DNS_TIMEOUT: Duration = Duration::from_secs(10);
/// TCP/TLS connect deadline.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Additional deadline for receiving response headers after connect.
pub const RESPONSE_HEADER_TIMEOUT: Duration = Duration::from_secs(10);
/// Maximum idle duration for each response-body read.
pub const BODY_IDLE_TIMEOUT: Duration = Duration::from_secs(5);
/// End-to-end deadline including DNS, connect, headers and body streaming.
pub const TOTAL_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const USER_AGENT_VALUE: &str = "ZetaWeb/1";
const ACCEPT_VALUE: &str =
    "text/html, application/json, text/plain, text/markdown, image/jpeg, image/png, image/webp";
const PAYMENT_SIGNATURE: reqwest::header::HeaderName =
    reqwest::header::HeaderName::from_static("payment-signature");
const PAYMENT_RESPONSE: reqwest::header::HeaderName =
    reqwest::header::HeaderName::from_static("payment-response");
const MAX_PAYMENT_SIGNATURE_BYTES: usize = 16 * 1024;

/// Opaque validated payment header accepted only by the one-shot paid broker.
pub struct WebPaymentSignature(String);

impl WebPaymentSignature {
    /// Copies a canonical x402 header into the network boundary.
    ///
    /// # Errors
    ///
    /// Rejects empty, excessive or non-ASCII header values.
    pub fn new(value: &str) -> Result<Self, WebBrokerError> {
        if value.is_empty() || value.len() > MAX_PAYMENT_SIGNATURE_BYTES || !value.is_ascii() {
            return Err(WebBrokerError::ClientUnavailable);
        }
        HeaderValue::from_str(value).map_err(|_| WebBrokerError::ClientUnavailable)?;
        Ok(Self(value.to_owned()))
    }

    #[cfg(debug_assertions)]
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// Cloneable host cancellation signal for DNS, connection and body streaming.
#[derive(Clone, Default)]
pub struct WebCancellation {
    inner: Arc<CancellationInner>,
}

#[derive(Default)]
struct CancellationInner {
    cancelled: AtomicBool,
    notify: Notify,
}

impl WebCancellation {
    /// Marks the operation cancelled and wakes every active phase.
    pub fn cancel(&self) {
        if !self.inner.cancelled.swap(true, Ordering::SeqCst) {
            self.inner.notify.notify_waiters();
        }
    }

    /// Returns whether cancellation was already requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    pub(crate) async fn cancelled(&self) {
        loop {
            let notified = self.inner.notify.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

/// Sanitized bounded broker failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WebBrokerError {
    /// Host cancellation won before another terminal outcome.
    Cancelled,
    /// Host refused final paid-request admission before any send.
    SubmissionRejected,
    /// DNS did not finish inside its phase deadline.
    DnsDeadline,
    /// The canonical target failed WEB-002 resolution or public validation.
    TargetRejected(TargetValidationError),
    /// The application-owned HTTP client could not be constructed.
    ClientUnavailable,
    /// Connection or response headers did not complete under their deadlines.
    ConnectionOrHeaderDeadline,
    /// The response body stopped making progress.
    BodyIdleDeadline,
    /// The end-to-end operation exceeded its total deadline.
    TotalDeadline,
    /// Response framing, metadata, media or body validation failed closed.
    ResponseRejected(ResponseValidationError),
    /// A sanitized HTTP/TLS/transport failure occurred.
    TransportFailed,
}

impl fmt::Display for WebBrokerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Cancelled => "web retrieval was cancelled",
            Self::SubmissionRejected => "paid request was cancelled before submission",
            Self::DnsDeadline => "web target resolution timed out",
            Self::TargetRejected(_) => "web target was rejected",
            Self::ClientUnavailable => "web client is unavailable",
            Self::ConnectionOrHeaderDeadline => "web response did not start in time",
            Self::BodyIdleDeadline => "web response body stalled",
            Self::TotalDeadline => "web retrieval exceeded its total deadline",
            Self::ResponseRejected(_) => "web response was rejected",
            Self::TransportFailed => "web transport failed",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for WebBrokerError {}

/// Validated host-only response admitted for later bounded extraction.
///
/// Ordinary raw headers are discarded before this value is created. Only the
/// bounded x402 protocol header values needed by the trusted payment adapter
/// are retained. Untrusted body bytes and URL-bearing provenance have no
/// serialization or debug projection.
pub struct WebBrokerResponse {
    provenance: WebResponseProvenance,
    body: Vec<u8>,
    payment_required: Option<WebPaymentRequiredChallenge>,
    payment_required_headers: Vec<Vec<u8>>,
    payment_response_headers: Vec<Vec<u8>>,
}

impl WebBrokerResponse {
    #[cfg(feature = "test-support")]
    pub(crate) fn from_validated_parts(
        provenance: WebResponseProvenance,
        body: Vec<u8>,
        payment_required: Option<WebPaymentRequiredChallenge>,
    ) -> Self {
        Self {
            provenance,
            body,
            payment_required,
            payment_required_headers: Vec::new(),
            payment_response_headers: Vec::new(),
        }
    }

    /// Returns the numeric HTTP status.
    #[must_use]
    pub const fn status(&self) -> u16 {
        self.provenance.status()
    }

    /// Returns the host-derived terminal HTTP classification.
    #[must_use]
    pub const fn kind(&self) -> WebResponseKind {
        WebResponseKind::from_status(self.status())
    }

    /// Returns trusted retrieval provenance.
    #[must_use]
    pub const fn provenance(&self) -> &WebResponseProvenance {
        &self.provenance
    }

    /// Returns the accepted untrusted representation for a later bounded extractor.
    #[must_use]
    pub fn untrusted_body(&self) -> &[u8] {
        &self.body
    }

    /// Returns bounded non-authoritative x402 challenge recognition.
    #[must_use]
    pub const fn payment_required(&self) -> Option<&WebPaymentRequiredChallenge> {
        self.payment_required.as_ref()
    }

    /// Every raw `PAYMENT-REQUIRED` value retained only for strict x402 handling.
    #[must_use]
    pub fn payment_required_headers(&self) -> &[Vec<u8>] {
        &self.payment_required_headers
    }

    /// Every raw `PAYMENT-RESPONSE` value retained only for strict x402 validation.
    #[must_use]
    pub fn payment_response_headers(&self) -> &[Vec<u8>] {
        &self.payment_response_headers
    }

    pub(crate) fn into_parts(self) -> (WebResponseProvenance, Vec<u8>) {
        (self.provenance, self.body)
    }

    #[cfg(test)]
    pub(crate) fn test_document(
        url: crate::CanonicalWebUrl,
        media_type: ValidatedWebMediaType,
        body: Vec<u8>,
    ) -> Self {
        let provenance = WebResponseProvenance::new(
            url,
            WebMethod::Get,
            200,
            Some(media_type),
            &body,
            1,
            Duration::ZERO,
        );
        Self {
            provenance,
            body,
            payment_required: None,
            payment_required_headers: Vec::new(),
            payment_response_headers: Vec::new(),
        }
    }
}

pub(crate) struct TransportResponse {
    status: u16,
    media_type: Option<ValidatedWebMediaType>,
    body: Vec<u8>,
    payment_required: Option<WebPaymentRequiredChallenge>,
    payment_required_headers: Vec<Vec<u8>>,
    payment_response_headers: Vec<Vec<u8>>,
}

impl TransportResponse {
    #[cfg(test)]
    pub(crate) const fn test(status: u16) -> Self {
        Self {
            status,
            media_type: None,
            body: Vec::new(),
            payment_required: None,
            payment_required_headers: Vec::new(),
            payment_response_headers: Vec::new(),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct BrokerPolicy {
    dns_timeout: Duration,
    connect_timeout: Duration,
    response_header_timeout: Duration,
    body_idle_timeout: Duration,
    total_timeout: Duration,
    max_encoded_body_bytes: usize,
    max_decoded_body_bytes: usize,
    max_remote_asset_encoded_body_bytes: usize,
    max_remote_asset_decoded_body_bytes: usize,
}

impl BrokerPolicy {
    const PRODUCTION: Self = Self {
        dns_timeout: DNS_TIMEOUT,
        connect_timeout: CONNECT_TIMEOUT,
        response_header_timeout: RESPONSE_HEADER_TIMEOUT,
        body_idle_timeout: BODY_IDLE_TIMEOUT,
        total_timeout: TOTAL_REQUEST_TIMEOUT,
        max_encoded_body_bytes: MAX_ENCODED_BODY_BYTES,
        max_decoded_body_bytes: MAX_DECODED_BODY_BYTES,
        max_remote_asset_encoded_body_bytes: MAX_REMOTE_ASSET_ENCODED_BODY_BYTES,
        max_remote_asset_decoded_body_bytes: MAX_REMOTE_ASSET_DECODED_BODY_BYTES,
    };

    #[cfg(test)]
    pub(crate) const fn test(
        dns_timeout: Duration,
        total_timeout: Duration,
        max_body_bytes: usize,
    ) -> Self {
        Self {
            dns_timeout,
            connect_timeout: total_timeout,
            response_header_timeout: total_timeout,
            body_idle_timeout: total_timeout,
            total_timeout,
            max_encoded_body_bytes: max_body_bytes,
            max_decoded_body_bytes: max_body_bytes,
            max_remote_asset_encoded_body_bytes: max_body_bytes,
            max_remote_asset_decoded_body_bytes: max_body_bytes,
        }
    }
}

pub(crate) trait WebTransport: Send + Sync {
    fn send<'a>(
        &'a self,
        request: &'a WebRequest,
        plan: &'a ValidatedConnectionPlan,
        cancellation: &'a WebCancellation,
        payment: Option<&'a WebPaymentSignature>,
        policy: BrokerPolicy,
    ) -> Pin<Box<dyn Future<Output = Result<TransportResponse, WebBrokerError>> + Send + 'a>>;
}

#[derive(Default)]
struct ReqwestTransport {
    admission: std::sync::Mutex<Option<Box<dyn FnOnce() -> bool + Send>>>,
}

impl ReqwestTransport {
    fn admit(&self) -> Result<(), WebBrokerError> {
        if let Some(admit) = self
            .admission
            .lock()
            .map_err(|_| WebBrokerError::SubmissionRejected)?
            .take()
            && !admit()
        {
            return Err(WebBrokerError::SubmissionRejected);
        }
        Ok(())
    }
}

impl WebTransport for ReqwestTransport {
    fn send<'a>(
        &'a self,
        request: &'a WebRequest,
        plan: &'a ValidatedConnectionPlan,
        cancellation: &'a WebCancellation,
        payment: Option<&'a WebPaymentSignature>,
        policy: BrokerPolicy,
    ) -> Pin<Box<dyn Future<Output = Result<TransportResponse, WebBrokerError>> + Send + 'a>> {
        Box::pin(async move {
            let client = build_client(plan, policy)?;
            let prepared = prepare_request(&client, request, payment)?;
            self.admit()?;
            let mut response = tokio::select! {
                () = cancellation.cancelled() => return Err(WebBrokerError::Cancelled),
                result = timeout(policy.response_header_timeout, client.execute(prepared)) => {
                    match result {
                        Err(_) => return Err(WebBrokerError::ConnectionOrHeaderDeadline),
                        Ok(Err(error)) if error.is_timeout() => {
                            return Err(WebBrokerError::ConnectionOrHeaderDeadline);
                        }
                        Ok(Err(_)) => return Err(WebBrokerError::TransportFailed),
                        Ok(Ok(response)) => response,
                    }
                }
            };

            let status_line_bytes = response_status_line_bytes(&response);
            let metadata = validate_response_metadata(
                request.method(),
                response.status(),
                status_line_bytes,
                response.headers(),
                policy.max_encoded_body_bytes,
                policy.max_remote_asset_encoded_body_bytes,
            )
            .map_err(WebBrokerError::ResponseRejected)?;
            let (payment_required_headers, payment_response_headers) =
                retained_payment_headers(&response);
            let ValidatedResponseMetadata {
                status,
                disposition,
                declared_length,
                max_body_bytes,
                payment_required,
            } = metadata;
            let media_type = match disposition {
                BodyDisposition::Accept(media_type) => media_type,
                BodyDisposition::Discard(media_type) => {
                    return Ok(TransportResponse {
                        status,
                        media_type,
                        body: Vec::new(),
                        payment_required,
                        payment_required_headers,
                        payment_response_headers,
                    });
                }
            };
            let mut accumulator = BodyAccumulator::new(
                media_type,
                declared_length,
                max_body_bytes,
                if media_type.is_image() {
                    policy.max_remote_asset_decoded_body_bytes
                } else {
                    policy.max_decoded_body_bytes
                },
            );
            if request.method() != WebMethod::Head {
                loop {
                    let chunk = tokio::select! {
                        () = cancellation.cancelled() => return Err(WebBrokerError::Cancelled),
                        result = timeout(policy.body_idle_timeout, response.chunk()) => {
                            match result {
                                Err(_) => return Err(WebBrokerError::BodyIdleDeadline),
                                Ok(Err(error)) if error.is_timeout() => {
                                    return Err(WebBrokerError::BodyIdleDeadline);
                                }
                                Ok(Err(_)) => return Err(WebBrokerError::TransportFailed),
                                Ok(Ok(chunk)) => chunk,
                            }
                        }
                    };
                    let Some(chunk) = chunk else {
                        break;
                    };
                    accumulator
                        .push_identity_chunk(&chunk)
                        .map_err(WebBrokerError::ResponseRejected)?;
                }
            }
            let body = accumulator
                .finish()
                .map_err(WebBrokerError::ResponseRejected)?;
            Ok(TransportResponse {
                status,
                media_type: Some(media_type),
                body,
                payment_required,
                payment_required_headers,
                payment_response_headers,
            })
        })
    }
}

fn retained_payment_headers(response: &reqwest::Response) -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
    let copy_values = |name: &reqwest::header::HeaderName| {
        response
            .headers()
            .get_all(name)
            .iter()
            .map(|value| value.as_bytes().to_vec())
            .collect()
    };
    let payment_required = if response.status().as_u16() == 402 {
        copy_values(&crate::response::PAYMENT_REQUIRED_HEADER)
    } else {
        Vec::new()
    };
    (payment_required, copy_values(&PAYMENT_RESPONSE))
}

/// Application-owned fixed-policy HTTPS broker.
#[derive(Clone, Copy, Default)]
pub struct WebBroker;

impl WebBroker {
    /// Resolves, validates and performs one exact host-approved request.
    ///
    /// The caller must consume matching AGENT-004 authorization before invoking
    /// this leaf. DNS begins inside this method; all later connection attempts
    /// are pinned to the validated WEB-002 address set.
    ///
    /// # Errors
    ///
    /// Returns a bounded sanitized outcome for cancellation, target rejection,
    /// phase/total deadline, unsupported encoding, body limit or transport
    /// failure.
    pub async fn execute(
        &self,
        request: &WebRequest,
        resolver: &impl HostResolver,
        cancellation: &WebCancellation,
    ) -> Result<WebBrokerResponse, WebBrokerError> {
        execute_with_transport(
            request,
            resolver,
            cancellation,
            &ReqwestTransport::default(),
            BrokerPolicy::PRODUCTION,
        )
        .await
    }

    /// Performs exactly one no-redirect, no-retry request with `PAYMENT-SIGNATURE`.
    ///
    /// The caller must already have consumed the exact single-use approval.
    ///
    /// # Errors
    ///
    /// Returns the same bounded broker failures as an unpaid request. Any
    /// failure is terminal for this invocation and is never retried here.
    pub async fn execute_paid_once(
        &self,
        request: &WebRequest,
        payment: &WebPaymentSignature,
        resolver: &impl HostResolver,
        cancellation: &WebCancellation,
    ) -> Result<WebBrokerResponse, WebBrokerError> {
        execute_with_payment_transport(
            request,
            resolver,
            cancellation,
            Some(payment),
            &ReqwestTransport::default(),
            BrokerPolicy::PRODUCTION,
        )
        .await
    }
    /// Performs a single paid request with final host admission after DNS and request construction.
    /// The callback receives no wallet authority or secret material from this leaf.
    /// # Errors
    /// Returns `SubmissionRejected` without sending when the host revokes admission.
    pub async fn execute_paid_once_with_admission(
        &self,
        request: &WebRequest,
        payment: &WebPaymentSignature,
        resolver: &impl HostResolver,
        cancellation: &WebCancellation,
        admit: impl FnOnce() -> bool + Send + 'static,
    ) -> Result<WebBrokerResponse, WebBrokerError> {
        let transport = ReqwestTransport {
            admission: std::sync::Mutex::new(Some(Box::new(admit))),
        };
        execute_with_payment_transport(
            request,
            resolver,
            cancellation,
            Some(payment),
            &transport,
            BrokerPolicy::PRODUCTION,
        )
        .await
    }
}

pub(crate) async fn execute_with_transport(
    request: &WebRequest,
    resolver: &impl HostResolver,
    cancellation: &WebCancellation,
    transport: &impl WebTransport,
    policy: BrokerPolicy,
) -> Result<WebBrokerResponse, WebBrokerError> {
    execute_with_payment_transport(request, resolver, cancellation, None, transport, policy).await
}

pub(crate) async fn execute_with_payment_transport(
    request: &WebRequest,
    resolver: &impl HostResolver,
    cancellation: &WebCancellation,
    payment: Option<&WebPaymentSignature>,
    transport: &impl WebTransport,
    policy: BrokerPolicy,
) -> Result<WebBrokerResponse, WebBrokerError> {
    if cancellation.is_cancelled() {
        return Err(WebBrokerError::Cancelled);
    }
    let started = Instant::now();
    let operation = async {
        let plan = tokio::select! {
            () = cancellation.cancelled() => return Err(WebBrokerError::Cancelled),
            result = timeout(
                policy.dns_timeout,
                crate::validate_public_target(request.url().clone(), resolver),
            ) => {
                match result {
                    Err(_) => return Err(WebBrokerError::DnsDeadline),
                    Ok(Err(error)) => return Err(WebBrokerError::TargetRejected(error)),
                    Ok(Ok(plan)) => plan,
                }
            }
        };
        let response = transport
            .send(request, &plan, cancellation, payment, policy)
            .await?;
        let retrieved_at_unix_millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX);
        let provenance = WebResponseProvenance::new(
            request.url().clone(),
            request.method(),
            response.status,
            response.media_type,
            &response.body,
            retrieved_at_unix_millis,
            started.elapsed(),
        );
        Ok(WebBrokerResponse {
            provenance,
            body: response.body,
            payment_required: response.payment_required,
            payment_required_headers: response.payment_required_headers,
            payment_response_headers: response.payment_response_headers,
        })
    };

    tokio::select! {
        () = cancellation.cancelled() => Err(WebBrokerError::Cancelled),
        result = timeout(policy.total_timeout, operation) => {
            result.unwrap_or(Err(WebBrokerError::TotalDeadline))
        }
    }
}

fn build_client(
    plan: &ValidatedConnectionPlan,
    policy: BrokerPolicy,
) -> Result<Client, WebBrokerError> {
    let addresses = plan.socket_addresses().collect::<Vec<_>>();

    Client::builder()
        .no_proxy()
        .redirect(redirect::Policy::none())
        .retry(reqwest::retry::never())
        .referer(false)
        .https_only(true)
        .http1_only()
        .min_tls_version(reqwest::tls::Version::TLS_1_2)
        .connect_timeout(policy.connect_timeout)
        .read_timeout(policy.body_idle_timeout)
        .timeout(policy.total_timeout)
        .pool_max_idle_per_host(0)
        .connection_verbose(false)
        .resolve_to_addrs(plan.server_name(), &addresses)
        .build()
        .map_err(|_| WebBrokerError::ClientUnavailable)
}

fn prepare_request(
    client: &Client,
    request: &WebRequest,
    payment: Option<&WebPaymentSignature>,
) -> Result<reqwest::Request, WebBrokerError> {
    let method = match request.method() {
        WebMethod::Get => Method::GET,
        WebMethod::Head => Method::HEAD,
        WebMethod::Post => Method::POST,
    };
    let mut builder = client
        .request(method, request.url().as_str())
        .headers(fixed_headers());
    if request.method() == WebMethod::Post {
        builder = builder
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(request.body().to_vec());
    }
    if let Some(payment) = payment {
        builder = builder.header(&PAYMENT_SIGNATURE, payment.0.as_str());
    }
    let prepared = builder
        .build()
        .map_err(|_| WebBrokerError::ClientUnavailable)?;
    if (request.method() != WebMethod::Post && prepared.body().is_some())
        || [AUTHORIZATION, PROXY_AUTHORIZATION, COOKIE, REFERER, ORIGIN]
            .iter()
            .any(|header| prepared.headers().contains_key(header))
    {
        return Err(WebBrokerError::ClientUnavailable);
    }
    Ok(prepared)
}

fn fixed_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static(USER_AGENT_VALUE));
    headers.insert(ACCEPT, HeaderValue::from_static(ACCEPT_VALUE));
    headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
    headers
}

fn response_status_line_bytes(response: &reqwest::Response) -> usize {
    let version_bytes: usize = match response.version() {
        reqwest::Version::HTTP_2 | reqwest::Version::HTTP_3 => 0,
        _ => 8,
    };
    let reason_bytes = response
        .extensions()
        .get::<hyper::ext::ReasonPhrase>()
        .map_or(0, |reason| reason.as_bytes().len().saturating_add(1));
    version_bytes
        .saturating_add(4)
        .saturating_add(reason_bytes)
        .saturating_add(2)
}

#[cfg(test)]
pub(crate) fn prepared_request_for_test(
    request: &WebRequest,
    plan: &ValidatedConnectionPlan,
) -> Result<reqwest::Request, WebBrokerError> {
    let client = build_client(plan, BrokerPolicy::PRODUCTION)?;
    prepare_request(&client, request, None)
}

#[cfg(test)]
pub(crate) fn prepared_paid_request_for_test(
    request: &WebRequest,
    plan: &ValidatedConnectionPlan,
    payment: &WebPaymentSignature,
) -> Result<reqwest::Request, WebBrokerError> {
    let client = build_client(plan, BrokerPolicy::PRODUCTION)?;
    prepare_request(&client, request, Some(payment))
}

#[cfg(test)]
#[path = "broker/paid_admission_tests.rs"]
mod paid_admission_tests;
