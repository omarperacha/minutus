//! Private post-collection fixtures test finalisation, not source/proof minting.
use super::*;
use alloy_consensus::Header;
use alloy_primitives::B256;
use serde_json::{Value, json};
use std::{
    io::{self, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

fn pending(server: &Server, satisfied: bool, selected_at: Instant) -> PendingGenericConditions {
    PendingGenericConditions {
        cohort: GenericLiveCohort {
            network: EvmNetwork::EthereumMainnet,
            block_hash: server.hash.clone(),
            block_number: 100,
            selected_at,
        },
        source_commitments: vec!["a".repeat(64)],
        observations: vec![],
        satisfied,
    }
}

#[test]
fn pending_conditions_require_final_canonical_anchor_before_any_certificate_or_false_result()
-> Result<(), Box<dyn std::error::Error>> {
    for satisfied in [false, true] {
        for reorg in [false, true] {
            let server = Server::start(reorg)?;
            let pending = pending(&server, satisfied, Instant::now());
            assert_eq!(
                server.requests.load(Ordering::Acquire),
                0,
                "collection data is not final proof"
            );
            let cancelled = AtomicBool::new(false);
            let control = SimulationControl::start(&cancelled);
            let result = pending.finalise(&server.endpoint, &control);
            if reorg {
                assert!(matches!(result, Err(GenericError::SnapshotMismatch)));
            } else if satisfied {
                let verified = result?;
                assert_eq!(verified.block_hash(), server.hash);
                assert!(verified.is_fresh());
            } else {
                assert!(matches!(result, Err(GenericError::ConditionUnsatisfied)));
            }
            assert!(
                server.finish()? >= 7,
                "even a false predicate must finish anchor checks"
            );
        }
    }
    Ok(())
}

#[test]
fn original_selection_clock_and_cancellation_cannot_be_renewed_by_finalisation()
-> Result<(), Box<dyn std::error::Error>> {
    for stale in [false, true] {
        let server = Server::start(false)?;
        let selected_at = if stale {
            Instant::now()
                .checked_sub(Duration::from_secs(31))
                .ok_or("test clock")?
        } else {
            Instant::now()
        };
        let pending = pending(&server, true, selected_at);
        assert_eq!(pending.cohort.selected_at, selected_at);
        let cancelled = AtomicBool::new(!stale);
        let control = SimulationControl::start(&cancelled);
        assert!(pending.finalise(&server.endpoint, &control).is_err());
        assert_eq!(server.finish()?, 0, "no stale or cancelled proof query");
    }
    Ok(())
}

#[test]
fn cancellation_after_canonical_conditions_still_blocks_the_full_completion_boundary()
-> Result<(), Box<dyn std::error::Error>> {
    let server = Server::start(false)?;
    let pending = pending(&server, true, Instant::now());
    let cancelled = AtomicBool::new(false);
    let control = SimulationControl::start(&cancelled);
    let observed = pending.finalise(&server.endpoint, &control)?;
    assert!(observed.is_fresh());
    // A callback or diagnostic delay can happen after condition finalisation.
    // The full-completion boundary must observe cancellation independently.
    cancelled.store(true, Ordering::Release);
    assert_eq!(
        super::super::final_control(&control, &observed),
        Err(GenericError::Expired)
    );
    assert!(server.finish()? >= 7);
    Ok(())
}

struct Server {
    endpoint: EvmRpcUrl,
    port: u16,
    hash: String,
    requests: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<io::Result<()>>>,
}
impl Server {
    fn start(reorg: bool) -> io::Result<Self> {
        let header = Header {
            number: 100,
            gas_used: 50_000,
            ..Header::default()
        };
        let mut changed = header.clone();
        changed.parent_hash = B256::repeat_byte(7);
        let mut original = serde_json::to_value(&header)?;
        original["hash"] = json!(format!("{:#x}", header.hash_slow()));
        let mut alternate = serde_json::to_value(&changed)?;
        alternate["hash"] = json!(format!("{:#x}", changed.hash_slow()));
        let hash = format!("{:#x}", header.hash_slow());
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(AtomicUsize::new(0));
        let (stopping, observed) = (Arc::clone(&stop), Arc::clone(&requests));
        let original_hash = hash.clone();
        let join = thread::spawn(move || {
            while !stopping.load(Ordering::Acquire) {
                let (mut stream, _) = match listener.accept() {
                    Ok(value) => value,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                if stopping.load(Ordering::Acquire) {
                    break;
                }
                stream.set_nonblocking(false)?;
                stream.set_read_timeout(Some(Duration::from_secs(1)))?;
                stream.set_write_timeout(Some(Duration::from_secs(1)))?;
                let request = read(&mut stream)?;
                observed.fetch_add(1, Ordering::AcqRel);
                let result = match request["method"].as_str() {
                    Some("eth_chainId") => json!("0x1"),
                    Some("eth_getBlockByHash") if request["params"][0] == original_hash => {
                        original.clone()
                    }
                    Some("eth_getBlockByHash" | "eth_getBlockByNumber") => {
                        if reorg {
                            alternate.clone()
                        } else {
                            original.clone()
                        }
                    }
                    _ => return Err(io::Error::other("unexpected proof RPC")),
                };
                let body = json!({"jsonrpc":"2.0","id":1,"result":result}).to_string();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )?;
            }
            Ok(())
        });
        Ok(Self {
            endpoint: EvmRpcUrl::loopback_test(port),
            port,
            hash,
            requests,
            stop,
            join: Some(join),
        })
    }
    fn finish(mut self) -> Result<usize, Box<dyn std::error::Error>> {
        self.shutdown()?;
        Ok(self.requests.load(Ordering::Acquire))
    }
    fn shutdown(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(join) = self.join.take() {
            join.join().map_err(|_| "fixture server panicked")??;
        }
        Ok(())
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}
fn read(stream: &mut TcpStream) -> io::Result<Value> {
    let mut bytes = Vec::new();
    loop {
        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            let headers = std::str::from_utf8(&bytes[..end]).map_err(io::Error::other)?;
            let length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .ok_or_else(|| io::Error::other("missing HTTP body length"))?;
            if bytes.len() >= end + 4 + length {
                return serde_json::from_slice(&bytes[end + 4..end + 4 + length])
                    .map_err(io::Error::other);
            }
        }
        let mut part = [0; 2048];
        let count = stream.read(&mut part)?;
        if count == 0 || bytes.len() + count > 16 * 1024 {
            return Err(io::Error::other("bounded fixture read failed"));
        }
        bytes.extend_from_slice(&part[..count]);
    }
}
