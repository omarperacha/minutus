use super::*;
use crate::EvmNetwork;
use alloy_primitives::U256;
use alloy_primitives::{Address, B256, keccak256};
use alloy_trie::{EMPTY_ROOT_HASH, HashBuilder, Nibbles, TrieAccount, proof::ProofRetainer};
use evm_simulation::{SimulationControl, SimulationError, SimulationStateSource, StateQuery};
use serde_json::{Value, json};
use std::{cell::RefCell, collections::VecDeque};
use std::{sync::atomic::AtomicBool, time::Duration};

fn trie(key: B256, value: &[u8]) -> (B256, Vec<String>) {
    let key = Nibbles::unpack(key);
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
fn account_fixture() -> (Address, B256, Value) {
    let address = Address::repeat_byte(0x11);
    let account = TrieAccount {
        nonce: 3,
        balance: U256::from(42),
        ..TrieAccount::default()
    };
    let (root, nodes) = trie(keccak256(address), &alloy_rlp::encode(account));
    (
        address,
        root,
        json!({"address":format!("{address:#x}"),"nonce":"0x3","balance":"0x2a","storageHash":format!("{:#x}",account.storage_root),"codeHash":format!("{:#x}",account.code_hash),"accountProof":nodes,"storageProof":[]}),
    )
}
#[test]
fn account_proofs_reject_changed_identity_values_and_roots() -> Result<(), SimulationError> {
    let (address, root, value) = account_fixture();
    assert!(proof::account(root, address, &value)?.0);
    assert!(proof::account(B256::ZERO, address, &value).is_err());
    assert!(proof::account(root, Address::repeat_byte(0x22), &value).is_err());
    for (field, replacement) in [
        ("balance", json!("0x2b")),
        ("nonce", json!("0x4")),
        ("codeHash", json!(format!("{:#x}", B256::ZERO))),
        ("accountProof", json!([])),
    ] {
        let mut changed = value.clone();
        changed[field] = replacement;
        assert!(proof::account(root, address, &changed).is_err(), "{field}");
    }
    Ok(())
}
#[test]
fn proved_absence_is_distinct_from_missing_state_and_false_zero() -> Result<(), SimulationError> {
    let address = Address::repeat_byte(0x11);
    let mut absent = json!({"address":format!("{address:#x}"),"nonce":"0x0","balance":"0x0","storageHash":format!("{EMPTY_ROOT_HASH:#x}"),"codeHash":format!("{:#x}",keccak256([])),"accountProof":[],"storageProof":[]});
    assert!(!proof::account(EMPTY_ROOT_HASH, address, &absent)?.0);
    absent["balance"] = json!("0x1");
    assert!(proof::account(EMPTY_ROOT_HASH, address, &absent).is_err());
    assert!(proof::account(EMPTY_ROOT_HASH, address, &Value::Null).is_err());
    Ok(())
}
#[test]
fn storage_values_require_exact_slot_and_valid_inclusion_or_absence() -> Result<(), SimulationError>
{
    let key = U256::from(3);
    let word = U256::from(42);
    let (root, nodes) = trie(keccak256(key.to_be_bytes::<32>()), &alloy_rlp::encode(word));
    let mut value = json!({"storageProof":[{"key":"0x3","value":"0x2a","proof":nodes}]});
    assert_eq!(proof::storage(root, key, &value)?, word);
    assert!(proof::storage(root, U256::from(4), &value).is_err());
    value["storageProof"][0]["value"] = json!("0x0");
    assert!(proof::storage(root, key, &value).is_err());
    let empty = json!({"storageProof":[{"key":"0x3","value":"0x0","proof":[]}]});
    assert_eq!(proof::storage(EMPTY_ROOT_HASH, key, &empty)?, U256::ZERO);
    Ok(())
}
fn header() -> alloy_consensus::Header {
    alloy_consensus::Header {
        number: 26_000_000,
        timestamp: 1_793_300_000,
        gas_limit: 30_000_000,
        base_fee_per_gas: Some(1),
        excess_blob_gas: Some(0),
        ..alloy_consensus::Header::default()
    }
}
fn header_json(header: &alloy_consensus::Header) -> Result<Value, Box<dyn std::error::Error>> {
    let mut value = serde_json::to_value(header)?;
    value["hash"] = json!(format!("{:#x}", header.hash_slow()));
    Ok(value)
}
#[test]
fn header_fields_must_hash_to_the_claimed_block_identity() -> Result<(), Box<dyn std::error::Error>>
{
    let header = header();
    let value = header_json(&header)?;
    assert!(header::decode(value.clone(), Some(header.hash_slow())).is_ok());
    let mut tampered = value;
    tampered["stateRoot"] = json!(format!("{:#x}", B256::repeat_byte(0x44)));
    assert!(matches!(
        header::decode(tampered, Some(header.hash_slow())),
        Err(SimulationError::InconsistentState)
    ));
    Ok(())
}
struct Fixture {
    responses: RefCell<VecDeque<(&'static str, Value, Value)>>,
}
impl source::StateRpc for Fixture {
    fn value(
        &self,
        method: &str,
        params: &Value,
        timeout: Duration,
    ) -> Result<Value, SimulationError> {
        assert!(timeout <= Duration::from_secs(1));
        let (expected, arguments, value) = self
            .responses
            .borrow_mut()
            .pop_front()
            .ok_or(SimulationError::StateUnavailable)?;
        assert_eq!(method, expected);
        assert_eq!(params, &arguments);
        Ok(value)
    }
}
#[test]
fn cancellation_reorg_and_invalid_proof_never_issue_fallback_or_later_code_fetch()
-> Result<(), Box<dyn std::error::Error>> {
    let header = header();
    let hash = header.hash_slow();
    let address = Address::repeat_byte(0x11);
    let fixture = Fixture {
        responses: RefCell::new(VecDeque::from([(
            "eth_getProof",
            json!([format!("{address:#x}"),[],{"blockHash":format!("{hash:#x}"),"requireCanonical":true}]),
            Value::Null,
        )])),
    };
    let mut source =
        source::StateSource::new(fixture, EvmNetwork::EthereumMainnet, header.clone(), hash);
    let cancelled = AtomicBool::new(true);
    let query = StateQuery::Account {
        address: format!("{address:#x}"),
    };
    assert!(matches!(
        source.query(&query, &SimulationControl::start(&cancelled)),
        Err(SimulationError::Cancelled)
    ));
    assert_eq!(source.rpc.responses.borrow().len(), 1);
    cancelled.store(false, std::sync::atomic::Ordering::Release);
    assert!(
        source
            .query(&query, &SimulationControl::start(&cancelled))
            .is_err()
    );
    assert!(source.rpc.responses.borrow().is_empty());
    let mut changed = header.clone();
    changed.state_root = B256::repeat_byte(0x44);
    let fixture = Fixture {
        responses: RefCell::new(VecDeque::from([
            ("eth_chainId", json!([]), json!("0x1")),
            (
                "eth_getBlockByNumber",
                json!([format!("0x{:x}", header.number), false]),
                header_json(&changed)?,
            ),
        ])),
    };
    let mut source = source::StateSource::new(fixture, EvmNetwork::EthereumMainnet, header, hash);
    assert!(matches!(
        source.canonical(&SimulationControl::start(&cancelled)),
        Err(SimulationError::InconsistentState)
    ));
    Ok(())
}

#[test]
fn cancellation_observed_after_rpc_return_prevents_code_fetch() {
    struct CancelAfter<'a>(&'a AtomicBool, std::cell::Cell<u32>);
    impl source::StateRpc for CancelAfter<'_> {
        fn value(&self, _: &str, _: &Value, _: Duration) -> Result<Value, SimulationError> {
            self.1.set(self.1.get() + 1);
            self.0.store(true, std::sync::atomic::Ordering::Release);
            Ok(Value::Null)
        }
    }
    let cancelled = AtomicBool::new(false);
    let h = header();
    let hash = h.hash_slow();
    let mut state = source::StateSource::new(
        CancelAfter(&cancelled, std::cell::Cell::new(0)),
        EvmNetwork::EthereumMainnet,
        h,
        hash,
    );
    let result = state.query(
        &StateQuery::Account {
            address: format!("{:#x}", Address::repeat_byte(0x11)),
        },
        &SimulationControl::start(&cancelled),
    );
    assert!(matches!(result, Err(SimulationError::Cancelled)));
    assert_eq!(state.rpc.1.get(), 1);
}

#[test]
fn amsterdam_header_slot_and_access_list_hash_are_exactly_bound()
-> Result<(), Box<dyn std::error::Error>> {
    let mut h = header();
    h.timestamp = 1_791_294_816;
    h.slot_number = Some(42);
    h.block_access_list_hash = Some(B256::repeat_byte(0x55));
    let hash = h.hash_slow();
    let value = header_json(&h)?;
    let (_, block) = header::decode(value.clone(), Some(hash))?;
    assert_eq!(block.slot_number, Some(42));
    assert_eq!(
        block.block_access_list_hash,
        Some(format!("{:#x}", B256::repeat_byte(0x55)))
    );
    for (field, replacement) in [
        ("slotNumber", json!("0x2b")),
        ("blockAccessListHash", json!(format!("{:#x}", B256::ZERO))),
    ] {
        let mut changed = value.clone();
        changed[field] = replacement;
        assert!(matches!(
            header::decode(changed, Some(hash)),
            Err(SimulationError::InconsistentState)
        ));
    }
    Ok(())
}

#[test]
fn malformed_maximum_parent_height_rejects_without_overflow_or_fallback()
-> Result<(), Box<dyn std::error::Error>> {
    let mut parent = header();
    parent.number = u64::MAX;
    let parent_hash = parent.hash_slow();
    let mut current = header();
    current.number = 10;
    current.parent_hash = parent_hash;
    let hash = current.hash_slow();
    let fixture = Fixture {
        responses: RefCell::new(VecDeque::from([(
            "eth_getBlockByHash",
            json!([format!("{parent_hash:#x}"), false]),
            header_json(&parent)?,
        )])),
    };
    let mut source = source::StateSource::new(fixture, EvmNetwork::EthereumMainnet, current, hash);
    let cancelled = AtomicBool::new(false);
    assert!(matches!(
        source.query(
            &StateQuery::BlockHash { number: 9 },
            &SimulationControl::start(&cancelled)
        ),
        Err(SimulationError::InconsistentState)
    ));
    assert!(source.rpc.responses.borrow().is_empty());
    Ok(())
}
