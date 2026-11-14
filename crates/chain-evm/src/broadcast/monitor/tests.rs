//! Only loopback public observations; the fixture accepts no submission method.
use super::*;
use crate::{BaseNetwork, EvmRpcUrl, TransactionHash, TransactionLifecycle};
use serde_json::{Value, json};
use std::{
    io::{self, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const SENDER: &str = "0x2222222222222222222222222222222222222222";
fn hash() -> Result<TransactionHash, crate::BroadcastError> {
    TransactionHash::new(&format!("0x{}", "11".repeat(32)))
}
fn response(method: &'static str, result: Value) -> Reply {
    Reply {
        method,
        body: Value::Object(serde_json::Map::from_iter([
            ("jsonrpc".into(), Value::from("2.0")),
            ("id".into(), Value::from(1)),
            ("result".into(), result),
        ])),
        status: 200,
        delay: Duration::ZERO,
        cancel: false,
    }
}
fn chain() -> Reply {
    response("eth_chainId", json!("0x14a34"))
}

#[test]
fn null_receipt_is_normal_pending_and_legacy_monitor_keeps_its_interface()
-> Result<(), Box<dyn std::error::Error>> {
    let hash = hash()?;
    for bounded in [false, true] {
        let mut server = Server::new(
            vec![
                chain(),
                response("eth_getTransactionReceipt", Value::Null),
                response("eth_getTransactionByHash", json!({"hash":hash.as_str()})),
            ],
            None,
        )?;
        let endpoint = EvmRpcUrl::loopback_test(server.port);
        let observation = if bounded {
            monitor_transaction_bounded(
                BaseNetwork::Sepolia,
                &endpoint,
                &hash,
                SENDER,
                "7",
                Duration::from_secs(2),
                &AtomicBool::new(false),
            )?
        } else {
            monitor_transaction(BaseNetwork::Sepolia, &endpoint, &hash, SENDER, "7")?
        };
        assert_eq!(observation.lifecycle(), TransactionLifecycle::Pending);
        assert_eq!(observation.block_hash(), None);
        assert_eq!(
            server.finish()?,
            vec![
                "eth_chainId",
                "eth_getTransactionReceipt",
                "eth_getTransactionByHash"
            ]
        );
    }
    Ok(())
}

#[test]
fn null_receipt_and_transaction_keep_pending_or_nonce_advanced_observation_separate()
-> Result<(), Box<dyn std::error::Error>> {
    for (nonce, expected) in [
        ("0x7", TransactionLifecycle::Pending),
        ("0x8", TransactionLifecycle::DroppedOrReplaced),
    ] {
        let mut server = Server::new(
            vec![
                chain(),
                response("eth_getTransactionReceipt", Value::Null),
                response("eth_getTransactionByHash", Value::Null),
                response("eth_getTransactionCount", json!(nonce)),
            ],
            None,
        )?;
        let observation = monitor_transaction_bounded(
            BaseNetwork::Sepolia,
            &EvmRpcUrl::loopback_test(server.port),
            &hash()?,
            SENDER,
            "7",
            Duration::from_secs(2),
            &AtomicBool::new(false),
        )?;
        assert_eq!(observation.lifecycle(), expected);
        assert_eq!(server.finish()?.len(), 4);
    }
    Ok(())
}

#[test]
fn strict_chain_receipt_hash_status_and_envelope_faults_never_become_retryable_pending()
-> Result<(), Box<dyn std::error::Error>> {
    let hash = hash()?;
    let receipt = json!({"transactionHash":hash.as_str(),"blockNumber":"0x2a","blockHash":format!("0x{}","33".repeat(32)),"status":"0x1"});
    let mut wrong_hash = receipt.clone();
    wrong_hash["transactionHash"] = json!(format!("0x{}", "44".repeat(32)));
    let mut wrong_status = receipt.clone();
    wrong_status["status"] = json!("0x2");
    let mut ambiguous = response("eth_getTransactionReceipt", Value::Null);
    ambiguous.body["error"] = json!({"code":-32000,"message":"provider secret https://credential"});
    let missing = Reply {
        body: json!({"jsonrpc":"2.0","id":1}),
        ..response("eth_getTransactionReceipt", Value::Null)
    };
    for (replies, stage, cause) in [
        (
            vec![response("eth_chainId", json!("0x1"))],
            MonitorStage::ChainIdValidation,
            MonitorCause::ChainIdMismatch {
                expected: 84532,
                actual: 1,
            },
        ),
        (
            vec![chain(), response("eth_getTransactionReceipt", wrong_hash)],
            MonitorStage::ReceiptValidation,
            MonitorCause::TransactionHashMismatch,
        ),
        (
            vec![chain(), response("eth_getTransactionReceipt", wrong_status)],
            MonitorStage::ReceiptValidation,
            MonitorCause::MalformedResponse,
        ),
        (
            vec![chain(), ambiguous],
            MonitorStage::ReceiptRpc,
            MonitorCause::MalformedResponse,
        ),
        (
            vec![chain(), missing],
            MonitorStage::ReceiptRpc,
            MonitorCause::MalformedResponse,
        ),
    ] {
        let mut server = Server::new(replies, None)?;
        let error = monitor_transaction_bounded(
            BaseNetwork::Sepolia,
            &EvmRpcUrl::loopback_test(server.port),
            &hash,
            SENDER,
            "7",
            Duration::from_secs(2),
            &AtomicBool::new(false),
        )
        .err()
        .ok_or("invalid observation accepted")?;
        assert_eq!(error.stage(), stage);
        assert_eq!(error.cause(), cause);
        assert!(!error.retryable_read());
        assert!(!serde_json::to_string(&error)?.contains("credential"));
        assert!(!error.to_string().contains("provider secret"));
        server.finish()?;
    }
    Ok(())
}

#[test]
fn only_reviewed_transport_and_http_failures_allow_bounded_read_retry()
-> Result<(), Box<dyn std::error::Error>> {
    for (status, eligible) in [
        (408, true),
        (429, true),
        (503, true),
        (404, false),
        (401, false),
        (302, false),
    ] {
        let mut reply = response("eth_getTransactionReceipt", Value::Null);
        reply.status = status;
        let mut server = Server::new(vec![chain(), reply], None)?;
        let error = monitor_transaction_bounded(
            BaseNetwork::Sepolia,
            &EvmRpcUrl::loopback_test(server.port),
            &hash()?,
            SENDER,
            "7",
            Duration::from_secs(2),
            &AtomicBool::new(false),
        )
        .err()
        .ok_or("HTTP error accepted")?;
        assert_eq!(error.cause(), MonitorCause::HttpStatus { status });
        assert_eq!(error.retryable_read(), eligible);
        assert_eq!(server.finish()?.len(), 2);
    }
    let error = super::failure::MonitorFailure::new(
        MonitorStage::ReceiptRpc,
        MonitorCause::RpcRejected { code: -32000 },
        Duration::ZERO,
    );
    assert!(!error.retryable_read());
    for cause in [MonitorCause::Transport, MonitorCause::TimedOut] {
        assert!(
            super::failure::MonitorFailure::new(MonitorStage::ReceiptRpc, cause, Duration::ZERO)
                .retryable_read()
        );
    }
    Ok(())
}

#[test]
fn shrinking_whole_attempt_budget_and_cancellation_stop_before_another_rpc()
-> Result<(), Box<dyn std::error::Error>> {
    let requested = Arc::new(AtomicBool::new(false));
    let mut first = chain();
    first.cancel = true;
    let mut server = Server::new(vec![first], Some(Arc::clone(&requested)))?;
    let error = monitor_transaction_bounded(
        BaseNetwork::Sepolia,
        &EvmRpcUrl::loopback_test(server.port),
        &hash()?,
        SENDER,
        "7",
        Duration::from_secs(2),
        &requested,
    )
    .err()
    .ok_or("cancelled read accepted")?;
    assert_eq!(error.cause(), MonitorCause::Cancelled);
    assert_eq!(server.finish()?.len(), 1);
    let mut delayed = response("eth_getTransactionReceipt", Value::Null);
    delayed.delay = Duration::from_millis(350);
    let mut server = Server::new(vec![chain(), delayed], None)?;
    let began = Instant::now();
    let error = monitor_transaction_bounded(
        BaseNetwork::Sepolia,
        &EvmRpcUrl::loopback_test(server.port),
        &hash()?,
        SENDER,
        "7",
        Duration::from_millis(150),
        &AtomicBool::new(false),
    )
    .err()
    .ok_or("expired read accepted")?;
    assert!(began.elapsed() < Duration::from_secs(1));
    assert!(matches!(
        error.cause(),
        MonitorCause::BudgetElapsed | MonitorCause::TimedOut
    ));
    assert_eq!(error.stage(), MonitorStage::ReceiptRpc);
    assert_eq!(server.finish()?.len(), 2);
    Ok(())
}

#[test]
fn already_stopped_or_zero_budget_observations_do_not_start_transport()
-> Result<(), Box<dyn std::error::Error>> {
    let endpoint =
        EvmRpcUrl::new("https://user:secret@invalid.example/private?credential=hidden".into())?;
    for (remaining, cancelled, expected) in [
        (Duration::ZERO, false, MonitorCause::BudgetElapsed),
        (Duration::from_secs(1), true, MonitorCause::Cancelled),
    ] {
        let error = monitor_transaction_bounded(
            BaseNetwork::Sepolia,
            &endpoint,
            &hash()?,
            SENDER,
            "7",
            remaining,
            &AtomicBool::new(cancelled),
        )
        .err()
        .ok_or("stopped read accepted")?;
        assert_eq!(error.stage(), MonitorStage::InputValidation);
        assert_eq!(error.cause(), expected);
        assert!(!serde_json::to_string(&error)?.contains("secret"));
        assert!(!error.retryable_read());
    }
    Ok(())
}

struct Reply {
    method: &'static str,
    body: Value,
    status: u16,
    delay: Duration,
    cancel: bool,
}
struct Server {
    port: u16,
    stop: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<io::Result<Vec<String>>>>,
}
impl Server {
    fn new(replies: Vec<Reply>, cancelled: Option<Arc<AtomicBool>>) -> io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let join = thread::spawn(move || {
            let mut methods = Vec::new();
            for reply in replies {
                let Some(mut stream) = accept(&listener, &stopped)? else {
                    break;
                };
                let method = read_request(&mut stream)?;
                if method != reply.method {
                    return Err(io::Error::other("unexpected RPC method"));
                }
                methods.push(method);
                if reply.cancel
                    && let Some(cancelled) = &cancelled
                {
                    cancelled.store(true, Ordering::Release);
                }
                thread::sleep(reply.delay);
                let bytes = serde_json::to_vec(&reply.body)?;
                let header = format!(
                    "HTTP/1.1 {} Result\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    reply.status,
                    bytes.len()
                );
                for part in [header.as_bytes(), bytes.as_slice()] {
                    if let Err(error) = stream.write_all(part)
                        && !matches!(
                            error.kind(),
                            io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset
                        )
                    {
                        return Err(error);
                    }
                }
            }
            Ok(methods)
        });
        Ok(Self {
            port,
            stop,
            join: Some(join),
        })
    }
    fn finish(&mut self) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        self.join
            .take()
            .ok_or("server already joined")?
            .join()
            .map_err(|_| "server panicked")?
            .map_err(Into::into)
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
fn accept(listener: &TcpListener, stop: &AtomicBool) -> io::Result<Option<TcpStream>> {
    let began = Instant::now();
    loop {
        if stop.load(Ordering::Acquire) {
            return Ok(None);
        }
        match listener.accept() {
            Ok((stream, _)) => {
                if stop.load(Ordering::Acquire) {
                    return Ok(None);
                }
                stream.set_nonblocking(false)?;
                stream.set_read_timeout(Some(Duration::from_secs(1)))?;
                stream.set_write_timeout(Some(Duration::from_secs(1)))?;
                return Ok(Some(stream));
            }
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock
                    && began.elapsed() < Duration::from_secs(2) =>
            {
                thread::sleep(Duration::from_millis(2));
            }
            Err(error) => return Err(error),
        }
    }
}
fn read_request(stream: &mut TcpStream) -> io::Result<String> {
    let mut bytes = Vec::new();
    loop {
        if let Some(end) = bytes.windows(4).position(|value| value == b"\r\n\r\n") {
            let header = std::str::from_utf8(&bytes[..end]).map_err(io::Error::other)?;
            let length = header
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .ok_or_else(|| io::Error::other("missing size"))?;
            if bytes.len() >= end + 4 + length {
                let body: Value = serde_json::from_slice(&bytes[end + 4..end + 4 + length])?;
                return body["method"]
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| io::Error::other("missing method"));
            }
        }
        let mut buffer = [0; 2048];
        let count = stream.read(&mut buffer)?;
        if count == 0 || bytes.len() + count > 16 * 1024 {
            return Err(io::Error::other("invalid request"));
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
}
