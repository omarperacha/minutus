//! Full synthetic loopback JSON-RPC acquisition; never a public RPC or signer.
use super::*;
use alloy_consensus::{Signed, TxEip1559};
use alloy_primitives::{Signature, TxKind};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
#[derive(Clone, Copy)]
enum Fault {
    None,
    Raw,
    ReceiptIdentity,
    ReceiptRoot,
    Reorg,
    Large,
    Timeout,
    Cancel,
}
struct Fixture {
    block: Value,
    receipts: Vec<Value>,
    raw: Vec<Vec<u8>>,
    hashes: Vec<String>,
    changed: Value,
}
impl Fixture {
    fn new(count: usize) -> Result<Self, Box<dyn std::error::Error>> {
        let mut raw = Vec::new();
        let mut receipts = Vec::new();
        for index in 0..count {
            let transaction = TxEip1559 {
                chain_id: 1,
                nonce: u64::try_from(index)?,
                gas_limit: 100_000,
                max_fee_per_gas: 2,
                max_priority_fee_per_gas: 1,
                to: TxKind::Call(alloy_primitives::Address::repeat_byte(0x22)),
                input: vec![0x42; 32].into(),
                ..TxEip1559::default()
            };
            let signature = Signature::from_scalars_and_parity(
                B256::repeat_byte(1),
                B256::repeat_byte(2),
                false,
            );
            let signed = Signed::new_unhashed(transaction, signature);
            let mut bytes = Vec::new();
            signed.eip2718_encode(&mut bytes);
            raw.push(bytes);
            let mut value = receipt();
            value["cumulativeGasUsed"] = json!(format!("0x{:x}", 21_000 * (index + 1)));
            receipts.push(value);
        }
        let encoded = raw
            .iter()
            .zip(&receipts)
            .map(|(raw, value)| receipt_bytes(EvmNetwork::EthereumMainnet, raw, value))
            .collect::<Result<Vec<_>, _>>()?;
        let header = Header {
            number: 26_000_000,
            timestamp: 1_793_300_000,
            gas_limit: 30_000_000,
            gas_used: 1_000_000,
            base_fee_per_gas: Some(1),
            excess_blob_gas: Some(0),
            transactions_root: alloy_trie::root::ordered_trie_root_encoded(&raw),
            receipts_root: alloy_trie::root::ordered_trie_root_encoded(&encoded),
            ..Header::default()
        };
        let hash = format!("{:#x}", header.hash_slow());
        let hashes: Vec<_> = raw
            .iter()
            .map(|bytes| format!("{:#x}", keccak256(bytes)))
            .collect();
        let mut block = serde_json::to_value(&header)?;
        block["hash"] = json!(hash);
        block["transactions"] = json!(hashes);
        for (index, value) in receipts.iter_mut().enumerate() {
            value["transactionHash"] = json!(hashes[index]);
            value["transactionIndex"] = json!(format!("0x{index:x}"));
            value["blockHash"] = json!(hash);
            value["blockNumber"] = json!(format!("0x{:x}", header.number));
        }
        let mut alternate = header;
        alternate.parent_hash = B256::repeat_byte(0x44);
        let mut changed = serde_json::to_value(&alternate)?;
        changed["hash"] = json!(format!("{:#x}", alternate.hash_slow()));
        Ok(Self {
            block,
            receipts,
            raw,
            hashes,
            changed,
        })
    }
}
struct Server {
    port: u16,
    stop: Arc<AtomicBool>,
    requests: Arc<Mutex<Vec<String>>>,
    join: Option<thread::JoinHandle<()>>,
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
impl Server {
    fn new(
        fixture: Fixture,
        fault: Fault,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stopping = stop.clone();
        let observed = requests.clone();
        let join = thread::spawn(move || {
            let mut canonical_reads = 0;
            while !stopping.load(Ordering::Acquire) {
                let Ok((mut stream, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(2));
                    continue;
                };
                if stopping.load(Ordering::Acquire) {
                    break;
                }
                let Ok(request) = read_request(&mut stream) else {
                    continue;
                };
                let method = request["method"].as_str().unwrap_or("");
                if let Ok(mut requests) = observed.lock() {
                    requests.push(method.to_owned());
                }
                let index = request["params"][0]
                    .as_str()
                    .and_then(|hash| fixture.hashes.iter().position(|known| known == hash))
                    .unwrap_or(0);
                let result = match method {
                    "eth_chainId" => json!("0x1"),
                    "eth_getBlockByHash" => {
                        let mut block = fixture.block.clone();
                        if matches!(fault, Fault::Large) {
                            block["transactions"] = json!(vec![fixture.hashes[0].clone(); 65]);
                        }
                        block
                    }
                    "eth_getBlockByNumber" => {
                        canonical_reads += 1;
                        if matches!(fault, Fault::Reorg) && canonical_reads > 1 {
                            fixture.changed.clone()
                        } else {
                            fixture.block.clone()
                        }
                    }
                    "eth_getRawTransactionByHash" => {
                        if matches!(fault, Fault::Timeout) {
                            thread::sleep(Duration::from_millis(1200));
                        }
                        if matches!(fault, Fault::Cancel) {
                            cancelled.store(true, Ordering::Release);
                        }
                        let bytes = if matches!(fault, Fault::Raw) {
                            vec![2]
                        } else {
                            fixture.raw[index].clone()
                        };
                        json!(format!("0x{}", alloy_primitives::hex::encode(bytes)))
                    }
                    "eth_getTransactionReceipt" => {
                        let mut receipt = fixture.receipts[index].clone();
                        if matches!(fault, Fault::ReceiptIdentity) {
                            receipt["blockNumber"] = json!("0x1");
                        }
                        if matches!(fault, Fault::ReceiptRoot) {
                            receipt["status"] = json!("0x0");
                        }
                        receipt
                    }
                    _ => Value::Null,
                };
                let _ = respond(
                    &mut stream,
                    &json!({"jsonrpc":"2.0","id":1,"result":result}),
                );
            }
        });
        Ok(Self {
            port,
            stop,
            requests,
            join: Some(join),
        })
    }
}
fn read_request(stream: &mut TcpStream) -> Result<Value, Box<dyn std::error::Error>> {
    // Darwin accepted sockets inherit O_NONBLOCK from the listener. Switch
    // each stream to bounded blocking IO before reading fragmented HTTP bytes.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(1)))?;
    let mut bytes = Vec::new();
    let mut buffer = [0; 2048];
    let (start, length) = loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 || bytes.len() + read > 64 * 1024 {
            return Err("invalid fixture request".into());
        }
        bytes.extend(&buffer[..read]);
        if let Some(end) = bytes.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
            let headers = std::str::from_utf8(&bytes[..end])?;
            let length = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|length| length.trim().parse::<usize>().ok())
                })
                .ok_or("missing fixture request length")?;
            break (end + 4, length);
        }
    };
    while bytes.len() < start + length {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            return Err("truncated fixture request".into());
        }
        bytes.extend(&buffer[..read]);
    }
    Ok(serde_json::from_slice(&bytes[start..start + length])?)
}
fn respond(stream: &mut TcpStream, value: &Value) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = serde_json::to_vec(value)?;
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        bytes.len()
    )?;
    stream.write_all(&bytes)?;
    Ok(())
}
#[test]
fn full_rpc_acquisition_proves_both_roots_and_rejects_mutation_reorg_and_large_blocks()
-> Result<(), Box<dyn std::error::Error>> {
    for fault in [
        Fault::None,
        Fault::Raw,
        Fault::ReceiptIdentity,
        Fault::ReceiptRoot,
        Fault::Reorg,
        Fault::Large,
    ] {
        let fixture = Fixture::new(2)?;
        let hash = fixture.hashes[0].clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let server = Server::new(fixture, fault, cancelled.clone())?;
        let result = acquire_contract_transaction_proof(
            EvmNetwork::EthereumMainnet,
            &EvmRpcUrl::loopback_test(server.port),
            &hash,
            &cancelled,
        );
        if matches!(fault, Fault::None) {
            assert!(
                result.is_ok(),
                "acquisition failed: {:?}, requests={:?}",
                result.as_ref().err(),
                server.requests.lock().map_err(|_| "poisoned")?
            );
            let proof = result?;
            assert!(!proof.transaction_proof.is_empty());
            assert!(!proof.receipt_proof.is_empty());
        } else {
            assert!(result.is_err());
        }
        if matches!(fault, Fault::Large) {
            assert!(
                !server
                    .requests
                    .lock()
                    .map_err(|_| "poisoned fixture")?
                    .iter()
                    .any(|method| method == "eth_getRawTransactionByHash")
            );
        }
    }
    Ok(())
}
#[test]
fn transport_timeout_and_cancellation_are_bounded_and_terminal()
-> Result<(), Box<dyn std::error::Error>> {
    for fault in [Fault::Timeout, Fault::Cancel] {
        let fixture = Fixture::new(2)?;
        let hash = fixture.hashes[0].clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let server = Server::new(fixture, fault, cancelled.clone())?;
        let started = Instant::now();
        let result = acquire_contract_transaction_proof(
            EvmNetwork::EthereumMainnet,
            &EvmRpcUrl::loopback_test(server.port),
            &hash,
            &cancelled,
        );
        match fault {
            Fault::Cancel => assert!(
                matches!(
                    result,
                    Err(QualificationError::Simulation(SimulationError::Cancelled))
                ),
                "result={:?},requests={:?}",
                result.as_ref().err(),
                server.requests.lock().map_err(|_| "poisoned")?
            ),
            Fault::Timeout => assert!(matches!(
                result,
                Err(QualificationError::Simulation(
                    SimulationError::StateUnavailable
                ))
            )),
            _ => return Err("invalid timeout fixture".into()),
        }
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(
            server
                .requests
                .lock()
                .map_err(|_| "poisoned fixture")?
                .iter()
                .filter(|method| method.as_str() == "eth_getRawTransactionByHash")
                .count(),
            1
        );
    }
    Ok(())
}
