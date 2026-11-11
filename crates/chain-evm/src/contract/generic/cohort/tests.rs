//! Synthetic canonical header RPCs exercise the production live-anchor adapter.
use super::*;
use alloy_consensus::Header;
use alloy_primitives::B256;
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
    },
    thread,
};

struct Headers {
    endpoint: EvmRpcUrl,
    mode: Arc<AtomicU8>,
    stop: Arc<AtomicBool>,
    failures: Arc<Mutex<Vec<String>>>,
    join: Option<thread::JoinHandle<()>>,
}
impl Headers {
    fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let endpoint = EvmRpcUrl::loopback_test(listener.local_addr()?.port());
        let mode = Arc::new(AtomicU8::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let failures = Arc::new(Mutex::new(Vec::new()));
        let worker_failures = Arc::clone(&failures);
        let (worker_mode, worker_stop) = (Arc::clone(&mode), Arc::clone(&stop));
        let join = thread::spawn(move || {
            while !worker_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        if let Err(error) = serve(&mut stream, &worker_mode)
                            && let Ok(mut failures) = worker_failures.lock()
                        {
                            failures.push(error.to_string());
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            endpoint,
            mode,
            stop,
            failures,
            join: Some(join),
        })
    }
    fn checked<T>(
        &self,
        phase: &str,
        result: Result<T, GenericError>,
    ) -> Result<T, Box<dyn std::error::Error>> {
        let failures = self
            .failures
            .lock()
            .map_err(|_| "fixture error lock poisoned")?;
        if !failures.is_empty() {
            return Err(format!("{phase}: fixture request failure: {failures:?}").into());
        }
        result.map_err(|error| format!("{phase}: {error:?}").into())
    }
    fn healthy(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.checked("fixture finalisation", Ok(()))
    }
}
impl Drop for Headers {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
fn header(number: u64, reorg: bool) -> Header {
    Header {
        number,
        state_root: if reorg {
            B256::repeat_byte(1)
        } else {
            B256::ZERO
        },
        ..Header::default()
    }
}
fn serve(stream: &mut TcpStream, mode: &AtomicU8) -> Result<(), Box<dyn std::error::Error>> {
    // Darwin can inherit O_NONBLOCK from the listening socket. A read timeout
    // does not clear it: explicitly block accepted streams so request arrival
    // cannot race an immediate WouldBlock and silently discard the connection.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(1)))?;
    let request = read_request(stream)?;
    let mode = mode.load(Ordering::Acquire);
    let result = match request["method"].as_str() {
        Some("eth_chainId") => json!("0xaa36a7"),
        Some("eth_getBlockByHash") => {
            let number = (9..=11)
                .find(|number| {
                    request["params"][0] == format!("{:#x}", header(*number, false).hash_slow())
                })
                .ok_or("unknown hash")?;
            block_value(&header(number, false))?
        }
        Some("eth_getBlockByNumber") => {
            let number = if request["params"][0] == "latest" {
                match mode {
                    0 => 10,
                    2 => 9,
                    _ => 11,
                }
            } else {
                u64::from_str_radix(
                    request["params"][0]
                        .as_str()
                        .ok_or("height")?
                        .trim_start_matches("0x"),
                    16,
                )?
            };
            block_value(&header(number, mode == 3 && number == 10))?
        }
        _ => return Err("unexpected method".into()),
    };
    let body = serde_json::to_vec(&json!({"jsonrpc":"2.0","id":request["id"],"result":result}))?;
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(&body)?;
    Ok(())
}
fn block_value(header: &Header) -> Result<Value, serde_json::Error> {
    let mut value = serde_json::to_value(header)?;
    value["hash"] = json!(format!("{:#x}", header.hash_slow()));
    Ok(value)
}
fn read_request(stream: &mut TcpStream) -> Result<Value, Box<dyn std::error::Error>> {
    let mut bytes = Vec::new();
    loop {
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let length = std::str::from_utf8(&bytes[..end])?
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .ok_or("missing size")?;
            if bytes.len() >= end + 4 + length {
                return Ok(serde_json::from_slice(&bytes[end + 4..end + 4 + length])?);
            }
        }
        let mut buffer = [0; 4096];
        let count = stream.read(&mut buffer)?;
        if count == 0 || bytes.len() + count > 16 * 1024 {
            return Err("invalid request".into());
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
}
#[test]
fn production_anchor_accepts_descendant_but_rejects_rollback_and_reorg()
-> Result<(), Box<dyn std::error::Error>> {
    let server = Headers::start()?;
    let cancelled = AtomicBool::new(false);
    let cohort = server.checked(
        "initial latest selection",
        select_generic_live_cohort(&server.endpoint, EvmNetwork::EthereumSepolia, &cancelled),
    )?;
    assert_eq!(cohort.block_number(), 10);
    let selected_at = cohort.selected_at;
    server.mode.store(1, Ordering::Release);
    server.checked(
        "descendant canonical check",
        cohort.check(&server.endpoint, &SimulationControl::start(&cancelled)),
    )?;
    assert_eq!(cohort.selected_at, selected_at);
    server.mode.store(2, Ordering::Release);
    assert_eq!(
        cohort.check(&server.endpoint, &SimulationControl::start(&cancelled)),
        Err(GenericError::SnapshotMismatch)
    );
    server.mode.store(3, Ordering::Release);
    assert_eq!(
        cohort.check(&server.endpoint, &SimulationControl::start(&cancelled)),
        Err(GenericError::SnapshotMismatch)
    );
    server.healthy()?;
    Ok(())
}
#[test]
fn retaining_cohort_never_resets_selection_age() -> Result<(), GenericError> {
    let cohort = GenericLiveCohort {
        network: EvmNetwork::EthereumSepolia,
        block_hash: String::new(),
        block_number: 1,
        selected_at: Instant::now()
            .checked_sub(Duration::from_secs(31))
            .ok_or(GenericError::Limit)?,
    };
    assert!(!cohort.is_fresh());
    assert_eq!(cohort.retained().selected_at, cohort.selected_at);
    assert!(!cohort.retained().is_fresh());
    Ok(())
}
#[test]
fn cancelled_selection_cannot_mint_live_cohort() -> Result<(), Box<dyn std::error::Error>> {
    let server = Headers::start()?;
    assert!(
        select_generic_live_cohort(
            &server.endpoint,
            EvmNetwork::EthereumSepolia,
            &AtomicBool::new(true)
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn accepted_connection_waits_for_delayed_request_bytes() -> Result<(), Box<dyn std::error::Error>> {
    let server = Headers::start()?;
    let address = server.endpoint.public_origin().replacen("http://", "", 1);
    let mut stream = TcpStream::connect(address)?;
    stream.set_read_timeout(Some(Duration::from_secs(1)))?;
    // Let the server accept before providing any bytes. An inherited
    // nonblocking stream would immediately fail rather than await this request.
    thread::sleep(Duration::from_millis(20));
    let body = br#"{"jsonrpc":"2.0","id":1,"method":"eth_chainId","params":[]}"#;
    write!(
        stream,
        "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    let mut response = String::new();
    stream.take(16 * 1024).read_to_string(&mut response)?;
    assert!(response.contains("0xaa36a7"));
    server.healthy()?;
    Ok(())
}
