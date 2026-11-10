//! Numeric EIP-1186 transport never substitutes for exact-root verification.
use super::*;
use crate::EvmRpcUrl;
use alloy_trie::{HashBuilder, Nibbles, proof::ProofRetainer};
use std::{cell::RefCell, collections::VecDeque, sync::atomic::AtomicBool};

struct Snapshot {
    header: Header,
    proof: Value,
}
fn trie(key: B256, value: &[u8]) -> (B256, Vec<String>) {
    let path = Nibbles::unpack(key);
    let mut builder = HashBuilder::default().with_proof_retainer(ProofRetainer::from_iter([path]));
    builder.add_leaf(path, value);
    let root = builder.root();
    let nodes = builder
        .take_proof_nodes()
        .into_nodes_sorted()
        .into_iter()
        .map(|(_, bytes)| format!("{bytes:#x}"))
        .collect();
    (root, nodes)
}
fn snapshot(value: u64) -> Snapshot {
    let address = Address::repeat_byte(0x11);
    let (storage_root, storage) = trie(
        keccak256(U256::ZERO.to_be_bytes::<32>()),
        &alloy_rlp::encode(U256::from(value)),
    );
    let account = TrieAccount {
        nonce: 3,
        balance: U256::from(42),
        storage_root,
        ..TrieAccount::default()
    };
    let (state_root, nodes) = trie(keccak256(address), &alloy_rlp::encode(account));
    Snapshot {
        header: Header {
            number: 26_000_000,
            timestamp: 1_793_300_000,
            gas_limit: 30_000_000,
            base_fee_per_gas: Some(1),
            excess_blob_gas: Some(0),
            state_root,
            ..Header::default()
        },
        proof: json!({"address":format!("{address:#x}"),"nonce":"0x3","balance":"0x2a",
            "storageHash":format!("{storage_root:#x}"),"codeHash":format!("{:#x}",account.code_hash),
            "accountProof":nodes,"storageProof":[{"key":"0x0","value":format!("0x{value:x}"),"proof":storage}]}),
    }
}
struct Fixture {
    responses: RefCell<VecDeque<(&'static str, Value, Value)>>,
}
impl StateRpc for Fixture {
    fn value(
        &self,
        method: &str,
        params: &Value,
        timeout: Duration,
    ) -> Result<Value, SimulationError> {
        assert!(timeout <= Duration::from_secs(1));
        let (expected_method, expected_params, response) = self
            .responses
            .borrow_mut()
            .pop_front()
            .ok_or(SimulationError::StateUnavailable)?;
        assert_eq!(method, expected_method);
        assert_eq!(params, &expected_params);
        Ok(response)
    }
}
fn query() -> StateQuery {
    StateQuery::Storage {
        address: format!("{:#x}", Address::repeat_byte(0x11)),
        slot: format!("{:#066x}", U256::ZERO),
    }
}
fn parameters(header: &Header, slot: bool) -> Value {
    let slots = if slot {
        vec![format!("{:#066x}", U256::ZERO)]
    } else {
        vec![]
    };
    json!([
        format!("{:#x}", Address::repeat_byte(0x11)),
        slots,
        format!("0x{:x}", header.number)
    ])
}
fn code_parameters(hash: B256) -> Value {
    json!([format!("{:#x}",Address::repeat_byte(0x11)),{"blockHash":format!("{hash:#x}"),"requireCanonical":true}])
}

#[test]
fn numeric_proofs_keep_hash_bound_code_and_exact_canonical_finalisation()
-> Result<(), Box<dyn std::error::Error>> {
    let snapshot = snapshot(42);
    let hash = snapshot.header.hash_slow();
    let mut final_header = serde_json::to_value(&snapshot.header)?;
    final_header["hash"] = json!(format!("{hash:#x}"));
    let fixture = Fixture {
        responses: RefCell::new(VecDeque::from([
            (
                "eth_getProof",
                parameters(&snapshot.header, false),
                snapshot.proof.clone(),
            ),
            ("eth_getCode", code_parameters(hash), json!("0x")),
            (
                "eth_getProof",
                parameters(&snapshot.header, true),
                snapshot.proof,
            ),
            ("eth_chainId", json!([]), json!("0x1")),
            (
                "eth_getBlockByNumber",
                json!([format!("0x{:x}", snapshot.header.number), false]),
                final_header,
            ),
        ])),
    };
    let mut source = StateSource::new(fixture, EvmNetwork::EthereumMainnet, snapshot.header, hash);
    let cancelled = AtomicBool::new(false);
    let control = SimulationControl::start(&cancelled);
    let StateValue::Storage { value } = source.query(&query(), &control)? else {
        return Err("Expected exact storage proof".into());
    };
    assert_eq!(value, format!("{:#066x}", U256::from(42)));
    source.canonical(&control)?;
    assert!(source.rpc.responses.borrow().is_empty());
    assert_eq!(
        source
            .into_proved_state()
            .slot(Address::repeat_byte(0x11), U256::ZERO),
        Some(U256::from(42))
    );
    Ok(())
}

#[test]
fn valid_proof_for_another_root_at_the_same_height_rejects_without_fallback() {
    let pinned = snapshot(42);
    let other = snapshot(43);
    assert_eq!(pinned.header.number, other.header.number);
    assert_ne!(pinned.header.state_root, other.header.state_root);
    let fixture = Fixture {
        responses: RefCell::new(VecDeque::from([(
            "eth_getProof",
            parameters(&pinned.header, false),
            other.proof,
        )])),
    };
    let hash = pinned.header.hash_slow();
    let mut source = StateSource::new(fixture, EvmNetwork::EthereumMainnet, pinned.header, hash);
    let cancelled = AtomicBool::new(false);
    assert!(matches!(
        source.query(&query(), &SimulationControl::start(&cancelled)),
        Err(SimulationError::InconsistentState)
    ));
    assert!(source.frozen_facts().is_empty());
    assert_eq!(source.requests, 3);
    assert!(source.rpc.responses.borrow().is_empty());
}

#[test]
fn matching_account_proof_cannot_carry_a_foreign_storage_root() {
    let pinned = snapshot(42);
    let other = snapshot(43);
    let hash = pinned.header.hash_slow();
    let mut wrong_slot = pinned.proof.clone();
    wrong_slot["storageProof"] = other.proof["storageProof"].clone();
    let fixture = Fixture {
        responses: RefCell::new(VecDeque::from([
            (
                "eth_getProof",
                parameters(&pinned.header, false),
                pinned.proof,
            ),
            ("eth_getCode", code_parameters(hash), json!("0x")),
            ("eth_getProof", parameters(&pinned.header, true), wrong_slot),
        ])),
    };
    let mut source = StateSource::new(fixture, EvmNetwork::EthereumMainnet, pinned.header, hash);
    let cancelled = AtomicBool::new(false);
    assert!(matches!(
        source.query(&query(), &SimulationControl::start(&cancelled)),
        Err(SimulationError::InconsistentState)
    ));
    assert_eq!(source.frozen_facts().len(), 1);
    assert_eq!(source.requests, 5);
    assert!(source.rpc.responses.borrow().is_empty());
}

struct PublicProbeRpc<'a> {
    client: RpcClient<'a>,
}
impl StateRpc for PublicProbeRpc<'_> {
    fn value(
        &self,
        method: &str,
        params: &Value,
        timeout: Duration,
    ) -> Result<Value, SimulationError> {
        let started = std::time::Instant::now();
        let result = self
            .client
            .value_result_with_timeout(method, params, timeout);
        println!(
            "{}",
            json!({"method":method,"params":params,
            "elapsedMilliseconds":started.elapsed().as_millis(),
            "result":match &result {Ok(_)=>"received".to_owned(),Err(error)=>format!("{error:?}")}})
        );
        result.map_err(|_| SimulationError::StateUnavailable)
    }
}
fn public_probe_canonical(
    endpoint: &EvmRpcUrl,
    header: &Header,
    control: &SimulationControl<'_>,
) -> Result<(), SimulationError> {
    let rpc = PublicProbeRpc {
        client: RpcClient::new(endpoint).map_err(|_| SimulationError::StateUnavailable)?,
    };
    let call = |method, params| {
        rpc.value(
            method,
            &params,
            control.remaining()?.min(Duration::from_secs(1)),
        )
    };
    let chain = call("eth_chainId", json!([]))?;
    EvmNetwork::EthereumSepolia
        .validate_rpc_chain_id(chain.as_str().ok_or(SimulationError::InconsistentState)?)
        .map_err(|_| SimulationError::InconsistentState)?;
    let hash = header.hash_slow();
    let value = call("eth_getBlockByHash", json!([format!("{hash:#x}"), false]))?;
    let decoded: Header =
        serde_json::from_value(value).map_err(|_| SimulationError::InconsistentState)?;
    if decoded.hash_slow() != hash {
        return Err(SimulationError::InconsistentState);
    }
    let value = call(
        "eth_getBlockByNumber",
        json!([format!("0x{:x}", header.number), false]),
    )?;
    let decoded: Header =
        serde_json::from_value(value).map_err(|_| SimulationError::InconsistentState)?;
    if decoded.hash_slow() != hash {
        return Err(SimulationError::InconsistentState);
    }
    control.remaining().map(|_| ())
}

#[test]
#[ignore = "read-only fixed default public Sepolia RPC timing and proof sequence"]
fn default_public_sepolia_source_sequence_preserves_production_transport_bounds()
-> Result<(), Box<dyn std::error::Error>> {
    let endpoint = EvmRpcUrl::new(
        EvmNetwork::EthereumSepolia
            .descriptor()
            .default_rpc_url()
            .into(),
    )?;
    let cancelled = AtomicBool::new(false);
    let control = SimulationControl::start(&cancelled);
    let rpc = PublicProbeRpc {
        client: RpcClient::new(&endpoint).map_err(|error| format!("{error:?}"))?,
    };
    let value = rpc.value(
        "eth_getBlockByNumber",
        &json!(["latest", false]),
        control.remaining()?.min(Duration::from_secs(1)),
    )?;
    let claimed = value["hash"].as_str().ok_or("Missing public header hash")?;
    let header: Header = serde_json::from_value(value.clone())?;
    if format!("{:#x}", header.hash_slow()) != claimed {
        return Err("Wrong public header hash".into());
    }
    println!(
        "{}",
        json!({"mode":"fixedPublicReadOnly","height":header.number,
        "hash":claimed,"timestamp":header.timestamp,"stateRoot":format!("{:#x}",header.state_root)})
    );
    // latest_header then read_proved each performs this canonical check using
    // a new RpcClient. Preserve that exact production connection behaviour.
    public_probe_canonical(&endpoint, &header, &control)?;
    public_probe_canonical(&endpoint, &header, &control)?;
    let rpc = PublicProbeRpc {
        client: RpcClient::new(&endpoint).map_err(|error| format!("{error:?}"))?,
    };
    let hash = header.hash_slow();
    let mut source = StateSource::new(rpc, EvmNetwork::EthereumSepolia, header, hash);
    let target = "0x2738d13e81e30bc615766a0410e7cf199fd59a83";
    source.query(
        &StateQuery::Account {
            address: target.into(),
        },
        &control,
    )?;
    for slot in [
        "0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc",
        "0xb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103",
        "0xa3f0ad74e5423aebfd80d3ef4346578335a9a72aeaee59ff6cb3582b35133d50",
    ] {
        source.query(
            &StateQuery::Storage {
                address: target.into(),
                slot: slot.into(),
            },
            &control,
        )?;
    }
    source.canonical(&control)?;
    assert_eq!(source.frozen_facts().len(), 4);
    println!(
        "fixed public source sequence: account/code/control proofs and canonical root verified"
    );
    Ok(())
}
