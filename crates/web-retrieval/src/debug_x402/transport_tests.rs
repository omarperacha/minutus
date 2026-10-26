use std::{
    error::Error,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::TcpListener,
    sync::Notify,
    task::JoinHandle,
};

use super::*;

const BODY: &[u8] = br#"{"orderId":"test-order","paymentStatus":"paid"}"#;
const RECEIPT: &str = "test-settlement-header";

struct Server {
    request: DebugX402Request,
    received: Arc<Notify>,
    calls: Arc<AtomicUsize>,
    task: JoinHandle<std::io::Result<()>>,
}

impl Server {
    async fn start(header_delay: Duration, stall_body: bool) -> Result<Self, Box<dyn Error>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let base = format!("http://{}/x402", listener.local_addr()?);
        let scope = DebugX402Scope::parse(&base)?;
        let request = DebugX402Request::new(
            &scope,
            DebugX402Method::Post,
            &format!("{base}/purchase"),
            Vec::new(),
        )?;
        let received = Arc::new(Notify::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let notify = Arc::clone(&received);
        let count = Arc::clone(&calls);
        let task = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await?;
                count.fetch_add(1, Ordering::SeqCst);
                let mut request_bytes = Vec::new();
                let mut byte = [0; 1];
                while !request_bytes.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).await?;
                    request_bytes.push(byte[0]);
                    if request_bytes.len() > 4096 {
                        return Err(std::io::Error::other("test request header limit"));
                    }
                }
                notify.notify_one();
                tokio::time::sleep(header_delay).await;
                let headers = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nCache-Control: no-store\r\nPayment-Response: {RECEIPT}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    BODY.len()
                );
                if stream.write_all(headers.as_bytes()).await.is_err() {
                    continue;
                }
                if stall_body {
                    // Send enough body to finish header parsing, then stall.
                    stream.write_all(&BODY[..1]).await?;
                    tokio::time::sleep(Duration::from_mins(2)).await;
                } else {
                    let _ = stream.write_all(BODY).await;
                }
            }
        });
        Ok(Self {
            request,
            received,
            calls,
            task,
        })
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn synthetic_payment() -> Result<WebPaymentSignature, WebBrokerError> {
    // Transport-only fixture: no wallet, real authorization or merchant calls.
    WebPaymentSignature::new("eyJ4NDAyVmVyc2lvbiI6Mn0=")
}

#[tokio::test]
async fn paid_response_can_wait_past_discovery_deadline_and_preserves_receipt()
-> Result<(), Box<dyn Error>> {
    let paid = Server::start(Duration::from_secs(6), false).await?;
    let unpaid = Server::start(Duration::from_secs(6), false).await?;
    let cancellation = WebCancellation::default();
    let payment = synthetic_payment()?;
    let (paid_result, unpaid_result) = tokio::join!(
        DebugX402Broker.execute_request(&paid.request, None, Some(&payment), &cancellation),
        DebugX402Broker.execute_request(&unpaid.request, None, None, &cancellation),
    );
    let response = paid_result?;
    assert_eq!(response.status(), 200);
    assert_eq!(response.body(), BODY);
    assert_eq!(
        response.payment_response_headers(),
        &[RECEIPT.as_bytes().to_vec()]
    );
    assert_eq!(
        unpaid_result.err(),
        Some(WebBrokerError::ConnectionOrHeaderDeadline)
    );
    assert_eq!(paid.calls.load(Ordering::SeqCst), 1);
    assert_eq!(unpaid.calls.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn paid_header_and_total_deadlines_remain_terminal_without_retry()
-> Result<(), Box<dyn Error>> {
    let payment = synthetic_payment()?;
    for (deadlines, expected) in [
        (
            ResponseDeadlines {
                headers: Duration::from_millis(150),
                total: Duration::from_secs(2),
            },
            WebBrokerError::ConnectionOrHeaderDeadline,
        ),
        (
            ResponseDeadlines {
                headers: Duration::from_secs(2),
                total: Duration::from_millis(150),
            },
            WebBrokerError::TotalDeadline,
        ),
    ] {
        let server = Server::start(Duration::from_secs(1), false).await?;
        let result = DebugX402Broker
            .execute_with_deadlines(
                &server.request,
                None,
                Some(&payment),
                &WebCancellation::default(),
                deadlines,
            )
            .await;
        assert_eq!(result.err(), Some(expected));
        assert_eq!(server.calls.load(Ordering::SeqCst), 1);
    }
    Ok(())
}

#[tokio::test]
async fn paid_body_still_has_a_short_idle_deadline() -> Result<(), Box<dyn Error>> {
    let server = Server::start(Duration::ZERO, true).await?;
    let payment = synthetic_payment()?;
    let result = DebugX402Broker
        .execute_request(
            &server.request,
            None,
            Some(&payment),
            &WebCancellation::default(),
        )
        .await;
    assert_eq!(result.err(), Some(WebBrokerError::BodyIdleDeadline));
    assert_eq!(server.calls.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn cancellation_interrupts_the_longer_paid_response_wait() -> Result<(), Box<dyn Error>> {
    let server = Server::start(Duration::from_mins(2), false).await?;
    let cancellation = WebCancellation::default();
    let payment = synthetic_payment()?;
    let (result, ()) = tokio::join!(
        DebugX402Broker.execute_request(&server.request, None, Some(&payment), &cancellation),
        async {
            server.received.notified().await;
            cancellation.cancel();
        },
    );
    assert_eq!(result.err(), Some(WebBrokerError::Cancelled));
    assert_eq!(server.calls.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn final_paid_admission_rejection_does_not_connect() -> Result<(), Box<dyn Error>> {
    let server = Server::start(Duration::ZERO, false).await?;
    let payment = synthetic_payment()?;
    let result = DebugX402Broker
        .execute_request_with_admission(
            &server.request,
            None,
            &payment,
            &WebCancellation::default(),
            || false,
        )
        .await;
    assert_eq!(result.err(), Some(WebBrokerError::SubmissionRejected));
    assert_eq!(server.calls.load(Ordering::SeqCst), 0);
    Ok(())
}
