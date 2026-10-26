use std::{
    future::{Future, pending},
    net::{IpAddr, Ipv4Addr},
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use reqwest::header::{
    ACCEPT, ACCEPT_ENCODING, AUTHORIZATION, CONTENT_LENGTH, COOKIE, ORIGIN, PROXY_AUTHORIZATION,
    REFERER, USER_AGENT,
};

use crate::{
    CanonicalWebUrl, HostResolver, ResolveError, ResponseValidationError, WebBrokerError,
    WebCancellation, WebMethod, WebRequest, WebResponseKind,
    broker::{
        BrokerPolicy, TransportResponse, WebTransport, execute_with_payment_transport,
        execute_with_transport, prepared_paid_request_for_test, prepared_request_for_test,
    },
    validate_public_target,
};

struct PublicResolver {
    calls: AtomicUsize,
}

impl PublicResolver {
    const fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
        }
    }
}

impl HostResolver for PublicResolver {
    fn resolve<'a>(
        &'a self,
        _host: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<IpAddr>, ResolveError>> + Send + 'a>> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Box::pin(async { Ok(vec![IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))]) })
    }
}

struct PendingResolver {
    calls: AtomicUsize,
}

struct FixedAnswersResolver {
    answers: Vec<IpAddr>,
    calls: AtomicUsize,
}

impl FixedAnswersResolver {
    fn new(answers: Vec<IpAddr>) -> Self {
        Self {
            answers,
            calls: AtomicUsize::new(0),
        }
    }
}

impl HostResolver for FixedAnswersResolver {
    fn resolve<'a>(
        &'a self,
        _host: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<IpAddr>, ResolveError>> + Send + 'a>> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let answers = self.answers.clone();
        Box::pin(async move { Ok(answers) })
    }
}

struct RebindingResolver {
    calls: AtomicUsize,
}

impl HostResolver for RebindingResolver {
    fn resolve<'a>(
        &'a self,
        _host: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<IpAddr>, ResolveError>> + Send + 'a>> {
        let call = self.calls.fetch_add(1, Ordering::Relaxed);
        Box::pin(async move {
            if call == 0 {
                Ok(vec![IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))])
            } else {
                Ok(vec![IpAddr::V4(Ipv4Addr::LOCALHOST)])
            }
        })
    }
}

impl PendingResolver {
    const fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
        }
    }
}

impl HostResolver for PendingResolver {
    fn resolve<'a>(
        &'a self,
        _host: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<IpAddr>, ResolveError>> + Send + 'a>> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Box::pin(pending())
    }
}

struct ImmediateTransport {
    calls: AtomicUsize,
    status: u16,
}

impl ImmediateTransport {
    const fn new(status: u16) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            status,
        }
    }
}

impl WebTransport for ImmediateTransport {
    fn send<'a>(
        &'a self,
        _request: &'a WebRequest,
        _plan: &'a crate::ValidatedConnectionPlan,
        _cancellation: &'a WebCancellation,
        _payment: Option<&'a super::WebPaymentSignature>,
        _policy: BrokerPolicy,
    ) -> Pin<Box<dyn Future<Output = Result<TransportResponse, WebBrokerError>> + Send + 'a>> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let status = self.status;
        Box::pin(async move { Ok(TransportResponse::test(status)) })
    }
}

struct PendingTransport {
    calls: AtomicUsize,
}

struct InspectBoundPlanTransport {
    calls: AtomicUsize,
}

impl WebTransport for InspectBoundPlanTransport {
    fn send<'a>(
        &'a self,
        _request: &'a WebRequest,
        plan: &'a crate::ValidatedConnectionPlan,
        _cancellation: &'a WebCancellation,
        _payment: Option<&'a super::WebPaymentSignature>,
        _policy: BrokerPolicy,
    ) -> Pin<Box<dyn Future<Output = Result<TransportResponse, WebBrokerError>> + Send + 'a>> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let addresses = plan.socket_addresses().collect::<Vec<_>>();
        Box::pin(async move {
            assert_eq!(addresses.len(), 1);
            assert_eq!(
                addresses[0].ip(),
                IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))
            );
            Ok(TransportResponse::test(200))
        })
    }
}

struct FailureTransport {
    calls: AtomicUsize,
    error: WebBrokerError,
}

impl WebTransport for FailureTransport {
    fn send<'a>(
        &'a self,
        _request: &'a WebRequest,
        _plan: &'a crate::ValidatedConnectionPlan,
        _cancellation: &'a WebCancellation,
        _payment: Option<&'a super::WebPaymentSignature>,
        _policy: BrokerPolicy,
    ) -> Pin<Box<dyn Future<Output = Result<TransportResponse, WebBrokerError>> + Send + 'a>> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let error = self.error;
        Box::pin(async move { Err(error) })
    }
}

impl PendingTransport {
    const fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
        }
    }
}

impl WebTransport for PendingTransport {
    fn send<'a>(
        &'a self,
        _request: &'a WebRequest,
        _plan: &'a crate::ValidatedConnectionPlan,
        _cancellation: &'a WebCancellation,
        _payment: Option<&'a super::WebPaymentSignature>,
        _policy: BrokerPolicy,
    ) -> Pin<Box<dyn Future<Output = Result<TransportResponse, WebBrokerError>> + Send + 'a>> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Box::pin(pending())
    }
}

fn request(method: WebMethod) -> Result<WebRequest, crate::UrlValidationError> {
    Ok(WebRequest::new(
        method,
        CanonicalWebUrl::parse("https://public.example.com/report?secret=value")?,
    ))
}

#[tokio::test]
async fn prepared_wire_request_has_only_fixed_headers_and_no_body_or_credentials()
-> Result<(), Box<dyn std::error::Error>> {
    let resolver = PublicResolver::new();
    let web_request = request(WebMethod::Get)?;
    let plan = validate_public_target(web_request.url().clone(), &resolver).await?;
    let prepared = prepared_request_for_test(&web_request, &plan)?;

    assert_eq!(prepared.method(), reqwest::Method::GET);
    assert_eq!(prepared.url().as_str(), web_request.url().as_str());
    assert!(prepared.body().is_none());
    assert_eq!(prepared.headers().len(), 3);
    assert_eq!(prepared.headers()[USER_AGENT], "ZetaWeb/1");
    assert_eq!(
        prepared.headers()[ACCEPT],
        "text/html, application/json, text/plain, text/markdown, image/jpeg, image/png, image/webp"
    );
    assert_eq!(prepared.headers()[ACCEPT_ENCODING], "identity");
    for forbidden in [
        AUTHORIZATION,
        PROXY_AUTHORIZATION,
        COOKIE,
        REFERER,
        ORIGIN,
        CONTENT_LENGTH,
    ] {
        assert!(!prepared.headers().contains_key(forbidden));
    }
    Ok(())
}

#[tokio::test]
async fn paid_wire_request_adds_only_payment_signature_once()
-> Result<(), Box<dyn std::error::Error>> {
    let resolver = PublicResolver::new();
    let web_request = request(WebMethod::Get)?;
    let plan = validate_public_target(web_request.url().clone(), &resolver).await?;
    let payment = super::WebPaymentSignature::new("eyJ4NDAyVmVyc2lvbiI6Mn0=")?;
    let prepared = prepared_paid_request_for_test(&web_request, &plan, &payment)?;

    assert_eq!(
        prepared.headers()["payment-signature"],
        "eyJ4NDAyVmVyc2lvbiI6Mn0="
    );
    assert_eq!(
        prepared
            .headers()
            .get_all("payment-signature")
            .iter()
            .count(),
        1
    );
    assert!(prepared.body().is_none());
    for forbidden in [AUTHORIZATION, PROXY_AUTHORIZATION, COOKIE, REFERER, ORIGIN] {
        assert!(!prepared.headers().contains_key(forbidden));
    }
    Ok(())
}

#[tokio::test]
async fn paid_transport_loss_is_terminal_and_never_retried()
-> Result<(), Box<dyn std::error::Error>> {
    let resolver = PublicResolver::new();
    let transport = FailureTransport {
        calls: AtomicUsize::new(0),
        error: WebBrokerError::TransportFailed,
    };
    let payment = super::WebPaymentSignature::new("eyJ4NDAyVmVyc2lvbiI6Mn0=")?;
    let result = execute_with_payment_transport(
        &request(WebMethod::Get)?,
        &resolver,
        &WebCancellation::default(),
        Some(&payment),
        &transport,
        BrokerPolicy::test(Duration::from_secs(1), Duration::from_secs(1), 32),
    )
    .await;

    assert_eq!(result.err(), Some(WebBrokerError::TransportFailed));
    assert_eq!(resolver.calls.load(Ordering::Relaxed), 1);
    assert_eq!(transport.calls.load(Ordering::Relaxed), 1);
    Ok(())
}

#[tokio::test]
async fn head_request_has_no_body_and_uses_the_same_fixed_profile()
-> Result<(), Box<dyn std::error::Error>> {
    let resolver = PublicResolver::new();
    let web_request = request(WebMethod::Head)?;
    let plan = validate_public_target(web_request.url().clone(), &resolver).await?;
    let prepared = prepared_request_for_test(&web_request, &plan)?;

    assert_eq!(prepared.method(), reqwest::Method::HEAD);
    assert!(prepared.body().is_none());
    assert_eq!(prepared.headers()[ACCEPT_ENCODING], "identity");
    Ok(())
}

#[tokio::test]
async fn pre_cancelled_request_performs_no_dns_or_transport()
-> Result<(), Box<dyn std::error::Error>> {
    let resolver = PublicResolver::new();
    let transport = ImmediateTransport::new(200);
    let cancellation = WebCancellation::default();
    cancellation.cancel();

    let result = execute_with_transport(
        &request(WebMethod::Get)?,
        &resolver,
        &cancellation,
        &transport,
        BrokerPolicy::test(Duration::from_millis(100), Duration::from_millis(100), 1024),
    )
    .await;

    assert_eq!(result.err(), Some(WebBrokerError::Cancelled));
    assert_eq!(resolver.calls.load(Ordering::Relaxed), 0);
    assert_eq!(transport.calls.load(Ordering::Relaxed), 0);
    Ok(())
}

#[tokio::test]
async fn cancellation_during_dns_prevents_connection() -> Result<(), Box<dyn std::error::Error>> {
    let resolver = PendingResolver::new();
    let transport = ImmediateTransport::new(200);
    let cancellation = WebCancellation::default();
    let trigger = cancellation.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        trigger.cancel();
    });

    let result = execute_with_transport(
        &request(WebMethod::Get)?,
        &resolver,
        &cancellation,
        &transport,
        BrokerPolicy::test(Duration::from_secs(1), Duration::from_secs(1), 1024),
    )
    .await;

    assert_eq!(result.err(), Some(WebBrokerError::Cancelled));
    assert_eq!(resolver.calls.load(Ordering::Relaxed), 1);
    assert_eq!(transport.calls.load(Ordering::Relaxed), 0);
    Ok(())
}

#[tokio::test]
async fn dns_and_total_deadlines_are_independent_and_bounded()
-> Result<(), Box<dyn std::error::Error>> {
    let request = request(WebMethod::Get)?;
    let cancellation = WebCancellation::default();
    let immediate = ImmediateTransport::new(200);
    let dns_result = execute_with_transport(
        &request,
        &PendingResolver::new(),
        &cancellation,
        &immediate,
        BrokerPolicy::test(Duration::from_millis(10), Duration::from_millis(100), 1024),
    )
    .await;
    assert_eq!(dns_result.err(), Some(WebBrokerError::DnsDeadline));

    let pending = PendingTransport::new();
    let total_result = execute_with_transport(
        &request,
        &PublicResolver::new(),
        &cancellation,
        &pending,
        BrokerPolicy::test(Duration::from_millis(100), Duration::from_millis(10), 1024),
    )
    .await;
    assert_eq!(total_result.err(), Some(WebBrokerError::TotalDeadline));
    assert_eq!(pending.calls.load(Ordering::Relaxed), 1);
    Ok(())
}

#[tokio::test]
async fn cancellation_during_transport_is_terminal() -> Result<(), Box<dyn std::error::Error>> {
    let resolver = PublicResolver::new();
    let transport = PendingTransport::new();
    let cancellation = WebCancellation::default();
    let trigger = cancellation.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        trigger.cancel();
    });

    let result = execute_with_transport(
        &request(WebMethod::Get)?,
        &resolver,
        &cancellation,
        &transport,
        BrokerPolicy::test(Duration::from_secs(1), Duration::from_secs(1), 1024),
    )
    .await;

    assert_eq!(result.err(), Some(WebBrokerError::Cancelled));
    assert_eq!(transport.calls.load(Ordering::Relaxed), 1);
    Ok(())
}

#[tokio::test]
async fn redirect_status_is_returned_once_without_broker_follow_up()
-> Result<(), Box<dyn std::error::Error>> {
    let resolver = PublicResolver::new();
    let transport = ImmediateTransport::new(302);
    let response = execute_with_transport(
        &request(WebMethod::Get)?,
        &resolver,
        &WebCancellation::default(),
        &transport,
        BrokerPolicy::test(Duration::from_secs(1), Duration::from_secs(1), 1024),
    )
    .await?;

    assert_eq!(response.status(), 302);
    assert_eq!(response.kind(), WebResponseKind::RedirectBlocked);
    assert!(response.untrusted_body().is_empty());
    assert_eq!(
        response.provenance().final_origin(),
        "https://public.example.com"
    );
    assert_eq!(response.provenance().method(), WebMethod::Get);
    assert_eq!(response.provenance().byte_count(), 0);
    assert_eq!(
        response.provenance().content_sha256(),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert!(!response.provenance().truncated());
    assert_eq!(transport.calls.load(Ordering::Relaxed), 1);
    Ok(())
}

#[tokio::test]
async fn mixed_or_private_dns_answers_stop_before_the_transport()
-> Result<(), Box<dyn std::error::Error>> {
    for answers in [
        vec![IpAddr::V4(Ipv4Addr::LOCALHOST)],
        vec![
            IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 7)),
        ],
    ] {
        let resolver = FixedAnswersResolver::new(answers);
        let transport = ImmediateTransport::new(200);
        let result = execute_with_transport(
            &request(WebMethod::Get)?,
            &resolver,
            &WebCancellation::default(),
            &transport,
            BrokerPolicy::test(Duration::from_secs(1), Duration::from_secs(1), 1024),
        )
        .await;
        assert!(matches!(result, Err(WebBrokerError::TargetRejected(_))));
        assert_eq!(resolver.calls.load(Ordering::Relaxed), 1);
        assert_eq!(transport.calls.load(Ordering::Relaxed), 0);
    }
    Ok(())
}

#[tokio::test]
async fn connection_uses_the_single_validated_dns_snapshot_despite_rebinding()
-> Result<(), Box<dyn std::error::Error>> {
    let resolver = RebindingResolver {
        calls: AtomicUsize::new(0),
    };
    let transport = InspectBoundPlanTransport {
        calls: AtomicUsize::new(0),
    };
    execute_with_transport(
        &request(WebMethod::Get)?,
        &resolver,
        &WebCancellation::default(),
        &transport,
        BrokerPolicy::test(Duration::from_secs(1), Duration::from_secs(1), 1024),
    )
    .await?;
    assert_eq!(resolver.calls.load(Ordering::Relaxed), 1);
    assert_eq!(transport.calls.load(Ordering::Relaxed), 1);
    Ok(())
}

#[tokio::test]
async fn malformed_slow_endless_and_oversized_peer_failures_are_never_retried()
-> Result<(), Box<dyn std::error::Error>> {
    for error in [
        WebBrokerError::TransportFailed,
        WebBrokerError::ConnectionOrHeaderDeadline,
        WebBrokerError::BodyIdleDeadline,
        WebBrokerError::ResponseRejected(ResponseValidationError::HeaderAggregateLimit),
        WebBrokerError::ResponseRejected(ResponseValidationError::EncodedBodyLimit),
        WebBrokerError::ResponseRejected(ResponseValidationError::DecodedBodyLimit),
    ] {
        let transport = FailureTransport {
            calls: AtomicUsize::new(0),
            error,
        };
        let result = execute_with_transport(
            &request(WebMethod::Get)?,
            &PublicResolver::new(),
            &WebCancellation::default(),
            &transport,
            BrokerPolicy::test(Duration::from_millis(50), Duration::from_millis(50), 1024),
        )
        .await;
        assert_eq!(result.err(), Some(error));
        assert_eq!(transport.calls.load(Ordering::Relaxed), 1);
    }

    let payment = ImmediateTransport::new(402);
    let response = execute_with_transport(
        &request(WebMethod::Get)?,
        &PublicResolver::new(),
        &WebCancellation::default(),
        &payment,
        BrokerPolicy::test(Duration::from_millis(50), Duration::from_millis(50), 1024),
    )
    .await?;
    assert_eq!(response.kind(), WebResponseKind::PaymentRequired);
    assert_eq!(payment.calls.load(Ordering::Relaxed), 1);
    Ok(())
}

#[tokio::test]
async fn x402_post_retains_exact_body_and_fixed_headers_across_paid_retry()
-> Result<(), Box<dyn std::error::Error>> {
    let url = CanonicalWebUrl::parse("https://example.com/paid")?;
    let body = br#"{"item":"sample","quantity":1}"#.to_vec();
    let request = WebRequest::json_post(url.clone(), body.clone())?;
    let plan = validate_public_target(url.clone(), &PublicResolver::new()).await?;
    let unpaid = prepared_request_for_test(&request, &plan)?;
    let payment = crate::WebPaymentSignature::new("synthetic-payment")?;
    let paid = prepared_paid_request_for_test(&request, &plan, &payment)?;
    assert_eq!(unpaid.method(), reqwest::Method::POST);
    assert_eq!(unpaid.url(), paid.url());
    assert_eq!(
        unpaid.body().and_then(reqwest::Body::as_bytes),
        Some(body.as_slice())
    );
    assert_eq!(
        paid.body().and_then(reqwest::Body::as_bytes),
        Some(body.as_slice())
    );
    assert_eq!(paid.headers()["content-type"], "application/json");
    assert_eq!(paid.headers()["payment-signature"], "synthetic-payment");
    assert!(!unpaid.headers().contains_key("payment-signature"));
    for header in [AUTHORIZATION, PROXY_AUTHORIZATION, COOKIE, REFERER, ORIGIN] {
        assert!(!paid.headers().contains_key(header));
    }
    let changed = WebRequest::json_post(url, br#"{"item":"sample","quantity":2}"#.to_vec())?;
    assert_ne!(request.identity_sha256(), changed.identity_sha256());
    assert!(
        WebMethod::try_from("POST").is_err(),
        "ordinary web tool parsing must remain GET/HEAD only"
    );
    assert!(
        WebRequest::json_post(
            CanonicalWebUrl::parse("https://example.com/paid")?,
            b"[]".to_vec()
        )
        .is_err()
    );
    assert!(
        WebRequest::json_post(
            CanonicalWebUrl::parse("https://example.com/paid")?,
            vec![b' '; 32 * 1024 + 1]
        )
        .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn x402_post_rejects_private_dns_before_sending_the_body()
-> Result<(), Box<dyn std::error::Error>> {
    let resolver = FixedAnswersResolver::new(vec![IpAddr::V4(Ipv4Addr::LOCALHOST)]);
    let transport = ImmediateTransport::new(200);
    let request = WebRequest::json_post(
        CanonicalWebUrl::parse("https://example.com/order")?,
        br#"{"item":"sample"}"#.to_vec(),
    )?;
    let result = execute_with_transport(
        &request,
        &resolver,
        &WebCancellation::default(),
        &transport,
        BrokerPolicy::test(Duration::from_secs(1), Duration::from_secs(1), 1024),
    )
    .await;
    assert!(matches!(result, Err(WebBrokerError::TargetRejected(_))));
    assert_eq!(transport.calls.load(Ordering::Relaxed), 0);
    Ok(())
}
