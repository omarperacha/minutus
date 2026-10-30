//! Source-only factory exercised with actual RPC, canonical headers, creation /
//! receipt inclusions and current account/code MPT proof. No trusted-worker bypass.
use super::*;
use alloy_consensus::{Eip658Value, Header, Receipt, Signed, TxEip1559};
use alloy_dyn_abi::DynSolValue;
use alloy_primitives::{Address, B256, Bytes, Log, LogData, Signature, TxKind, U256, keccak256};
use alloy_trie::{HashBuilder, Nibbles, TrieAccount, proof::ProofRetainer};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
#[derive(Clone, Copy)]
enum Fault {
    None,
    Code,
    Nonce,
    Mint,
    Chain,
    Reorg,
    History,
    LatestRollback,
}
struct Fixture {
    network: EvmNetwork,
    target: Address,
    proof: DeploymentProof,
    creation: Value,
    current: Value,
    changed: Value,
    account: Value,
    code: String,
}
fn leaf(key: Nibbles, value: &[u8]) -> (B256, Vec<String>) {
    let mut builder = HashBuilder::default().with_proof_retainer(ProofRetainer::from_iter([key]));
    builder.add_leaf(key, value);
    let root = builder.root();
    (
        root,
        builder
            .take_proof_nodes()
            .into_nodes_sorted()
            .into_iter()
            .map(|(_, bytes)| format!("{bytes:#x}"))
            .collect(),
    )
}
fn header_json(header: &Header) -> Result<Value, serde_json::Error> {
    let mut value = serde_json::to_value(header)?;
    value["hash"] = json!(format!("{:#x}", header.hash_slow()));
    Ok(value)
}
impl Fixture {
    fn new(network: EvmNetwork, fault: Fault) -> Result<Self, Box<dyn std::error::Error>> {
        let holder = Address::repeat_byte(0x42);
        let supply = U256::from(17);
        let (target, raw) = creation_transaction(network, holder, supply)?;
        let minted = if matches!(fault, Fault::Mint) {
            supply + U256::from(1)
        } else {
            supply
        };
        let log = Log {
            address: target,
            data: LogData::new_unchecked(
                vec![
                    keccak256("Transfer(address,address,uint256)"),
                    B256::ZERO,
                    B256::from(U256::from_be_slice(holder.as_slice()).to_be_bytes::<32>()),
                ],
                Bytes::copy_from_slice(&minted.to_be_bytes::<32>()),
            ),
        };
        let mut receipt = vec![2];
        receipt.extend(alloy_rlp::encode(
            Receipt {
                status: Eip658Value::Eip658(true),
                cumulative_gas_used: 1_000_000,
                logs: vec![log],
            }
            .with_bloom(),
        ));
        let key = Nibbles::unpack(alloy_rlp::encode(0u64));
        let (transactions_root, transaction_proof) = leaf(key, &raw);
        let (receipts_root, receipt_proof) = leaf(key, &receipt);
        let mut header = Header {
            number: 26_000_000,
            timestamp: 1_793_299_000,
            gas_limit: 30_000_000,
            gas_used: 1_000_000,
            base_fee_per_gas: Some(1),
            excess_blob_gas: Some(0),
            transactions_root,
            receipts_root,
            ..Header::default()
        };
        if network == EvmNetwork::EthereumSepolia {
            header.slot_number = Some(42);
            header.block_access_list_hash = Some(B256::repeat_byte(0x12));
        }
        let creation = header_json(&header)?;
        let proof = DeploymentProof {
            block_hash: format!("{:#x}", header.hash_slow()),
            transaction_index: 0,
            transaction: format!("0x{}", alloy_primitives::hex::encode(raw)),
            transaction_proof,
            receipt: format!("0x{}", alloy_primitives::hex::encode(receipt)),
            receipt_proof,
        };
        let code = if matches!(fault, Fault::Code) {
            vec![0]
        } else {
            alloy_primitives::hex::decode(
                include_str!("../../../../../profiles/erc20-oz-5.7.0/runtime.hex")
                    .trim()
                    .trim_start_matches("0x"),
            )?
        };
        let account = TrieAccount {
            nonce: if matches!(fault, Fault::Nonce) { 2 } else { 1 },
            code_hash: keccak256(&code),
            ..TrieAccount::default()
        };
        let (state_root, nodes) = leaf(
            Nibbles::unpack(keccak256(target)),
            &alloy_rlp::encode(account),
        );
        let account = json!({"address":format!("{target:#x}"),"balance":"0x0","nonce":format!("0x{:x}",account.nonce),"storageHash":format!("{:#x}",account.storage_root),"codeHash":format!("{:#x}",account.code_hash),"accountProof":nodes,"storageProof":[]});
        let mut current_header = header.clone();
        current_header.number += 10;
        current_header.timestamp += 1000;
        current_header.parent_hash = header.hash_slow();
        current_header.state_root = state_root;
        let current = header_json(&current_header)?;
        header.parent_hash = B256::repeat_byte(0x66);
        let changed = header_json(&header)?;
        Ok(Self {
            network,
            target,
            proof,
            creation,
            current,
            changed,
            account,
            code: format!("0x{}", alloy_primitives::hex::encode(code)),
        })
    }
}
struct Server {
    port: u16,
    stop: Arc<AtomicBool>,
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
    fn new(fixture: &Fixture, fault: Fault) -> Result<Self, Box<dyn std::error::Error>> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let network = fixture.network;
        let target = format!("{:#x}", fixture.target);
        let creation = fixture.creation.clone();
        let current = fixture.current.clone();
        let changed = fixture.changed.clone();
        let account = fixture.account.clone();
        let code = fixture.code.clone();
        let join = thread::spawn(move || {
            let mut latest_reads = 0;
            let mut code_read = false;
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
                let parameter = request["params"][0].as_str().unwrap_or("");
                let result = match request["method"].as_str().unwrap_or("") {
                    "eth_chainId" => json!(format!(
                        "0x{:x}",
                        network.descriptor().chain_id() + u64::from(matches!(fault, Fault::Chain))
                    )),
                    "eth_getBlockByHash" => {
                        if parameter == creation["hash"].as_str().unwrap_or("") {
                            if matches!(fault, Fault::History) {
                                Value::Null
                            } else {
                                creation.clone()
                            }
                        } else if parameter == current["hash"].as_str().unwrap_or("") {
                            current.clone()
                        } else {
                            Value::Null
                        }
                    }
                    "eth_getBlockByNumber" => {
                        if parameter == "latest" {
                            latest_reads += 1;
                            if matches!(fault, Fault::LatestRollback) && latest_reads > 1 {
                                creation.clone()
                            } else {
                                current.clone()
                            }
                        } else if parameter == creation["number"].as_str().unwrap_or("") {
                            if matches!(fault, Fault::Reorg) && code_read {
                                changed.clone()
                            } else {
                                creation.clone()
                            }
                        } else if parameter == current["number"].as_str().unwrap_or("") {
                            current.clone()
                        } else {
                            Value::Null
                        }
                    }
                    "eth_getProof" => {
                        if parameter == target
                            && request["params"][2]["blockHash"] == current["hash"]
                            && request["params"][2]["requireCanonical"] == true
                        {
                            account.clone()
                        } else {
                            Value::Null
                        }
                    }
                    "eth_getCode" => {
                        code_read = true;
                        if parameter == target
                            && request["params"][1]["blockHash"] == current["hash"]
                            && request["params"][1]["requireCanonical"] == true
                        {
                            json!(code)
                        } else {
                            Value::Null
                        }
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
                .ok_or("missing request length")?;
            break (end + 4, length);
        }
    };
    while bytes.len() < start + length {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            return Err("truncated request".into());
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
fn actual_source_factory_requires_creation_current_code_and_unchanged_canonical_tip()
-> Result<(), Box<dyn std::error::Error>> {
    for network in [
        EvmNetwork::BaseMainnet,
        EvmNetwork::BaseSepolia,
        EvmNetwork::EthereumMainnet,
        EvmNetwork::EthereumSepolia,
    ] {
        for fault in [
            Fault::None,
            Fault::Code,
            Fault::Nonce,
            Fault::Mint,
            Fault::Chain,
            Fault::Reorg,
            Fault::History,
            Fault::LatestRollback,
        ] {
            let fixture = Fixture::new(network, fault)?;
            let server = Server::new(&fixture, fault)?;
            let identity = qualify_contract_workflow_source(
                network,
                &EvmRpcUrl::loopback_test(server.port),
                &format!("{:#x}", fixture.target),
                &fixture.proof,
                &AtomicBool::new(false),
            );
            if matches!(fault, Fault::None) {
                let identity = identity?;
                assert_eq!(identity.network(), network);
                assert_eq!(identity.target(), format!("{:#x}", fixture.target));
                assert_eq!(
                    identity.block_hash(),
                    fixture.current["hash"].as_str().ok_or("missing hash")?
                );
            } else {
                assert!(identity.is_err());
            }
        }
    }
    Ok(())
}

fn creation_transaction(
    network: EvmNetwork,
    holder: Address,
    supply: U256,
) -> Result<(Address, Vec<u8>), Box<dyn std::error::Error>> {
    let mut input = alloy_primitives::hex::decode(
        include_str!("../../../../../profiles/erc20-oz-5.7.0/creation.hex")
            .trim()
            .trim_start_matches("0x"),
    )?;
    input.extend(
        DynSolValue::Tuple(vec![
            DynSolValue::String("Synthetic".into()),
            DynSolValue::String("SYN".into()),
            DynSolValue::Address(holder),
            DynSolValue::Uint(supply, 256),
        ])
        .abi_encode_params(),
    );
    let transaction = TxEip1559 {
        chain_id: network.descriptor().chain_id(),
        nonce: 3,
        gas_limit: 2_000_000,
        max_fee_per_gas: 10,
        max_priority_fee_per_gas: 1,
        to: TxKind::Create,
        input: input.into(),
        ..TxEip1559::default()
    };
    let r = B256::from_slice(&alloy_primitives::hex::decode(
        "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
    )?);
    let signed = Signed::new_unhashed(
        transaction,
        Signature::from_scalars_and_parity(r, B256::from(U256::from(1).to_be_bytes::<32>()), false),
    );
    let target = signed.recover_signer()?.create(3);
    let mut raw = Vec::new();
    signed.eip2718_encode(&mut raw);

    Ok((target, raw))
}
