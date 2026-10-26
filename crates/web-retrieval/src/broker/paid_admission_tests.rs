use crate::{
    CanonicalWebUrl, HostResolver, ResolveError, WebBrokerError, WebCancellation, WebMethod,
    WebPaymentSignature, WebRequest,
};
use std::{
    future::Future,
    net::{IpAddr, Ipv4Addr},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
struct ResolvingThenRevoking {
    revoked: Arc<AtomicBool>,
}
impl HostResolver for ResolvingThenRevoking {
    fn resolve<'a>(
        &'a self,
        _host: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<IpAddr>, ResolveError>> + Send + 'a>> {
        Box::pin(async move {
            tokio::task::yield_now().await;
            self.revoked.store(true, Ordering::Release);
            Ok(vec![IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))])
        })
    }
}
#[tokio::test]
async fn paid_admission_observes_revocation_during_dns_and_never_connects()
-> Result<(), Box<dyn std::error::Error>> {
    let revoked = Arc::new(AtomicBool::new(false));
    let called = Arc::new(AtomicBool::new(false));
    let resolver = ResolvingThenRevoking {
        revoked: revoked.clone(),
    };
    let request = WebRequest::new(
        WebMethod::Get,
        CanonicalWebUrl::parse("https://example.com/paid")?,
    );
    let payment = WebPaymentSignature::new("synthetic-payment-header")?;
    let check = called.clone();
    let observed = revoked.clone();
    let result = super::WebBroker
        .execute_paid_once_with_admission(
            &request,
            &payment,
            &resolver,
            &WebCancellation::default(),
            move || {
                check.store(true, Ordering::Release);
                !observed.load(Ordering::Acquire)
            },
        )
        .await;
    assert_eq!(result.err(), Some(WebBrokerError::SubmissionRejected));
    assert!(called.load(Ordering::Acquire));
    assert!(revoked.load(Ordering::Acquire));
    Ok(())
}

#[tokio::test]
async fn x402_post_final_admission_rejects_revocation_before_any_submission()
-> Result<(), Box<dyn std::error::Error>> {
    let revoked = Arc::new(AtomicBool::new(false));
    let resolver = ResolvingThenRevoking {
        revoked: revoked.clone(),
    };
    let request = WebRequest::json_post(
        CanonicalWebUrl::parse("https://example.com/paid")?,
        br#"{"item":"sample"}"#.to_vec(),
    )?;
    let payment = WebPaymentSignature::new("synthetic-payment-header")?;
    let result = super::WebBroker
        .execute_paid_once_with_admission(
            &request,
            &payment,
            &resolver,
            &WebCancellation::default(),
            move || !revoked.load(Ordering::Acquire),
        )
        .await;
    assert_eq!(result.err(), Some(WebBrokerError::SubmissionRejected));
    Ok(())
}
