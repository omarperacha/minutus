//! Full synthetic loopback JSON-RPC acquisition; never a public RPC or signer.
use super::*;
use alloy_consensus::{SignableTransaction, Signed, TxEip1559, TxEnvelope};
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
    ReorgAfterVerification,
    Large,
    Timeout,
    Cancel,
    ReceiptsUnsupported,
    ReceiptBatchRejected,
    ReceiptBatchMalformed,
    ReceiptBatchUnordered,
    AggregateBudget,
}
struct Fixture {
    block: Value,
    receipts: Vec<Value>,
    transactions: Vec<Value>,
    hashes: Vec<String>,
    changed: Value,
}
impl Fixture {
    fn new(count: usize) -> Result<Self, Box<dyn std::error::Error>> {
        Self::with_status(count, true)
    }
    fn with_status(count: usize, succeeded: bool) -> Result<Self, Box<dyn std::error::Error>> {
        let mut raw = Vec::new();
        let mut receipts = Vec::new();
        let mut transactions = Vec::new();
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
            transactions.push(serde_json::to_value(TxEnvelope::Eip1559(signed.clone()))?);
            let mut bytes = Vec::new();
            signed.eip2718_encode(&mut bytes);
            raw.push(bytes);
            let mut value = receipt();
            value["status"] = json!(if succeeded { "0x1" } else { "0x0" });
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
            gas_used: 21_000 * u64::try_from(count)?,
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
        for (index, value) in transactions.iter_mut().enumerate() {
            value["hash"] = json!(hashes[index]);
            value["transactionIndex"] = json!(format!("0x{index:x}"));
            value["blockHash"] = json!(hash);
            value["blockNumber"] = json!(format!("0x{:x}", header.number));
        }
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
            transactions,
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
                let value =
                    fixture_response(&fixture, fault, &request, &mut canonical_reads, &cancelled);
                let _ = respond(&mut stream, &value);
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

#[test]
fn standard_batches_cover_more_than_sixty_four_transactions_without_raw_rpc()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new(129)?;
    let hash = fixture.hashes[128].clone();
    let cancelled = Arc::new(AtomicBool::new(false));
    let server = Server::new(fixture, Fault::None, cancelled.clone())?;
    let proof = acquire_contract_transaction_proof(
        EvmNetwork::EthereumMainnet,
        &EvmRpcUrl::loopback_test(server.port),
        &hash,
        &cancelled,
    )?;
    assert_eq!(proof.transaction_index, 128);
    let requests = server.requests.lock().map_err(|_| "poisoned")?;
    assert_eq!(
        requests
            .iter()
            .filter(|method| method.as_str() == "eth_getBlockReceipts")
            .count(),
        1
    );
    assert_eq!(
        requests
            .iter()
            .filter(|method| method.as_str() == "eth_getTransactionReceipt")
            .count(),
        1
    );
    assert!(
        !requests
            .iter()
            .any(|method| method == "eth_getRawTransactionByHash")
    );
    Ok(())
}

#[test]
fn receipt_only_reconciliation_proves_exact_success_and_revert_without_submission()
-> Result<(), Box<dyn std::error::Error>> {
    for succeeded in [false, true] {
        let fixture = Fixture::with_status(1, succeeded)?;
        let (sender, digest) = expected_submission(&fixture)?;
        let hash = fixture.hashes[0].clone();
        let block_hash = fixture.block["hash"]
            .as_str()
            .ok_or("missing hash")?
            .to_owned();
        let cancelled = Arc::new(AtomicBool::new(false));
        let server = Server::new(fixture, Fault::None, cancelled.clone())?;
        let verified = crate::contract::verify_submitted_transaction(
            EvmNetwork::EthereumMainnet,
            &EvmRpcUrl::loopback_test(server.port),
            &hash,
            &sender,
            "0",
            &digest,
            &cancelled,
        )?;
        assert_eq!(verified.succeeded(), succeeded);
        assert_eq!(verified.block_hash(), block_hash);
        assert_eq!(verified.transaction_hash(), hash);
        assert_eq!(verified.signing_digest(), digest);
        let requests = server.requests.lock().map_err(|_| "poisoned")?;
        assert_eq!(
            requests
                .iter()
                .filter(|method| *method == "eth_getBlockByNumber")
                .count(),
            3
        );
        assert!(requests.iter().all(|method| matches!(
            method.as_str(),
            "eth_chainId"
                | "eth_getTransactionReceipt"
                | "eth_getBlockByHash"
                | "eth_getBlockByNumber"
                | "eth_getBlockReceipts"
        )));
    }
    Ok(())
}

#[test]
fn reconciliation_rechecks_canonicality_after_verification_and_retains_strict_failures()
-> Result<(), Box<dyn std::error::Error>> {
    for fault in [
        Fault::ReorgAfterVerification,
        Fault::ReceiptRoot,
        Fault::Raw,
        Fault::Cancel,
    ] {
        let fixture = Fixture::new(1)?;
        let (sender, digest) = expected_submission(&fixture)?;
        let hash = fixture.hashes[0].clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let server = Server::new(fixture, fault, cancelled.clone())?;
        assert!(
            crate::contract::verify_submitted_transaction(
                EvmNetwork::EthereumMainnet,
                &EvmRpcUrl::loopback_test(server.port),
                &hash,
                &sender,
                "0",
                &digest,
                &cancelled,
            )
            .is_err()
        );
    }
    Ok(())
}

fn expected_submission(fixture: &Fixture) -> Result<(String, String), Box<dyn std::error::Error>> {
    let TxEnvelope::Eip1559(signed) = serde_json::from_value(fixture.transactions[0].clone())?
    else {
        return Err("fixture is not type2".into());
    };
    Ok((
        format!("{:#x}", signed.recover_signer()?),
        format!("{:#x}", keccak256(signed.tx().encoded_for_signing())),
    ))
}

#[test]
fn receipt_fallback_requires_explicit_unsupported_and_charges_error_bodies()
-> Result<(), Box<dyn std::error::Error>> {
    for fault in [
        Fault::ReceiptsUnsupported,
        Fault::ReceiptBatchUnordered,
        Fault::ReceiptBatchRejected,
        Fault::ReceiptBatchMalformed,
        Fault::AggregateBudget,
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
        assert_eq!(
            result.is_ok(),
            matches!(
                fault,
                Fault::ReceiptsUnsupported | Fault::ReceiptBatchUnordered
            )
        );
        let requests = server.requests.lock().map_err(|_| "poisoned")?;
        assert_eq!(
            requests
                .iter()
                .filter(|method| method.as_str() == "eth_getTransactionReceipt")
                .count(),
            if matches!(fault, Fault::ReceiptsUnsupported | Fault::AggregateBudget) {
                2
            } else {
                1
            }
        );
        assert!(
            !requests
                .iter()
                .any(|method| method == "eth_getRawTransactionByHash")
        );
        if matches!(fault, Fault::AggregateBudget) {
            assert!(matches!(
                result,
                Err(QualificationError::Simulation(SimulationError::Limit))
            ));
        }
    }
    Ok(())
}
fn fixture_response(
    fixture: &Fixture,
    fault: Fault,
    request: &Value,
    canonical_reads: &mut u32,
    cancelled: &AtomicBool,
) -> Value {
    let method = request["method"].as_str().unwrap_or("");
    let index = request["params"][0]
        .as_str()
        .and_then(|hash| fixture.hashes.iter().position(|known| known == hash))
        .unwrap_or(0);
    if method == "eth_getBlockReceipts"
        && matches!(
            fault,
            Fault::ReceiptsUnsupported | Fault::ReceiptBatchRejected | Fault::AggregateBudget
        )
    {
        let code = if matches!(fault, Fault::ReceiptBatchRejected) {
            -32000
        } else {
            -32601
        };
        let padding = if matches!(fault, Fault::AggregateBudget) {
            "x".repeat(MAX_TOTAL / 2)
        } else {
            String::new()
        };
        return json!({"jsonrpc":"2.0","id":1,"error":{"code":code,"message":"fixture rejection","data":padding}});
    }
    let result = match method {
        "eth_chainId" => json!("0x1"),
        "eth_getBlockByHash" => {
            let mut block = fixture.block.clone();
            if request["params"][1] == true {
                if matches!(fault, Fault::Timeout) {
                    thread::sleep(Duration::from_millis(2200));
                }
                if matches!(fault, Fault::Cancel) {
                    cancelled.store(true, Ordering::Release);
                }
                block["transactions"] = json!(fixture.transactions);
                if matches!(fault, Fault::Raw) {
                    block["transactions"][0]["input"] = json!("0x01");
                }
                if matches!(fault, Fault::Large) {
                    block["transactions"] = json!(vec![fixture.transactions[0].clone(); 2049]);
                }
            }
            block
        }
        "eth_getBlockReceipts" => {
            if matches!(fault, Fault::ReceiptBatchMalformed) {
                json!([])
            } else if matches!(fault, Fault::ReceiptBatchUnordered) {
                json!(fixture.receipts.iter().rev().collect::<Vec<_>>())
            } else {
                json!(fixture.receipts)
            }
        }
        "eth_getBlockByNumber" => {
            *canonical_reads += 1;
            if (matches!(fault, Fault::Reorg) && *canonical_reads > 1)
                || (matches!(fault, Fault::ReorgAfterVerification) && *canonical_reads > 2)
            {
                fixture.changed.clone()
            } else {
                fixture.block.clone()
            }
        }
        "eth_getTransactionReceipt" => {
            let mut receipt = fixture.receipts[index].clone();
            if matches!(fault, Fault::AggregateBudget) && index != 0 {
                receipt["padding"] = json!("x".repeat(MAX_TOTAL / 2));
            }
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
    json!({"jsonrpc":"2.0","id":1,"result":result})
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
        assert!(started.elapsed() < Duration::from_secs(4));
        assert_eq!(
            server
                .requests
                .lock()
                .map_err(|_| "poisoned fixture")?
                .iter()
                .filter(|method| method.as_str() == "eth_getBlockByHash")
                .count(),
            1
        );
    }
    Ok(())
}
