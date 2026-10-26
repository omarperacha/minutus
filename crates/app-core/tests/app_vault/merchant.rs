//! Local synthetic merchant: no RPC, real settlement, credentials or funds.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::json;
use std::{
    io::{Read as _, Write as _},
    net::TcpListener,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};
pub(super) struct Merchant {
    pub(super) url: String,
    pub(super) origin: String,
    pub(super) calls: Arc<AtomicUsize>,
    pub(super) paid: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
impl Merchant {
    pub(super) fn start(account: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let origin = format!("http://{}", listener.local_addr()?);
        let url = format!("{origin}/x402/order");
        let challenge=STANDARD.encode(serde_json::to_vec(&json!({"x402Version":2,"resource":{"url":url},"accepts":[{"scheme":"exact","network":"eip155:84532","amount":"1","asset":"0x036cbd53842c5426634e7929541ec2318f3dcf7e","payTo":"0x209693bc6afc0c5328ba36faf03c514ef312287c","maxTimeoutSeconds":120,"extra":{"name":"USDC","version":"2"}}]}))?);
        let receipt=STANDARD.encode(serde_json::to_vec(&json!({"success":true,"transaction":format!("0x{}","ab".repeat(32)),"network":"eip155:84532","payer":account,"amount":"1"}))?);
        let stop = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(AtomicUsize::new(0));
        let paid = Arc::new(AtomicUsize::new(0));
        let (halt, total, paid_count) = (stop.clone(), calls.clone(), paid.clone());
        let account = account.to_owned();
        let worker = std::thread::spawn(move || {
            while !halt.load(Ordering::Acquire) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                };
                // macOS may inherit non-blocking mode from the listening socket.
                // Accepted fixture streams must wait for complete request headers.
                let _ = stream.set_nonblocking(false);
                total.fetch_add(1, Ordering::AcqRel);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let mut bytes = Vec::new();
                let mut byte = [0; 1];
                while !bytes.ends_with(b"\r\n\r\n") && bytes.len() < 16384 {
                    if stream.read_exact(&mut byte).is_err() {
                        return;
                    }
                    bytes.push(byte[0]);
                }
                let text = String::from_utf8_lossy(&bytes);
                let signature = text.lines().find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("payment-signature")
                        .then(|| value.trim())
                });
                let (status, protocol, body) = if let Some(header) = signature {
                    let value = STANDARD
                        .decode(header)
                        .ok()
                        .and_then(|v| serde_json::from_slice::<serde_json::Value>(&v).ok());
                    if value.as_ref().is_none_or(|v| {
                        v["payload"]["authorization"]["from"] != account
                            || v["accepted"]["amount"] != "1"
                    }) {
                        return;
                    }
                    paid_count.fetch_add(1, Ordering::AcqRel);
                    (
                        200,
                        format!("Payment-Response: {receipt}\r\n"),
                        "{\"fixture\":\"paid-order\"}",
                    )
                } else {
                    (402, format!("Payment-Required: {challenge}\r\n"), "{}")
                };
                let response = format!(
                    "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nCache-Control: no-store\r\n{protocol}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Ok(Self {
            url,
            origin,
            calls,
            paid,
            stop,
            worker: Some(worker),
        })
    }
}
impl Drop for Merchant {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
