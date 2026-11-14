//! Actual official-compiler + synthetic MPT + maintained replay qualification.
//! These ignored tests require a unit-test host with the normal compiler pin
//! section and sibling helper. They use no wallet, keys, RPC, signing or send.
use super::*;
use crate::local_simulation::proof;
use crate::{
    EvmNetwork,
    contract::generic::{
        GenericContractCandidate, GenericContractIdentity, GenericEffectOperation, GenericError,
        admit_generic_transition, generic_contract_intent,
    },
};
use alloy_consensus::Header;
use alloy_primitives::{B256, Bytes, keccak256};
use alloy_trie::{HashBuilder, Nibbles, TrieAccount, proof::ProofRetainer};
use evm_simulation::{SimulationBlock, SimulationNetwork, SimulationTransaction};
use serde_json::{Value, json};
use solidity_compilation::{CompilationInput, ReproducedArtifact, TrustedCompiler};
use std::{
    collections::BTreeMap,
    sync::{Arc, atomic::AtomicBool},
};

const SOURCE: &str = r#"// SPDX-License-Identifier: MIT
pragma solidity 0.8.38;
contract IndependentReviewFixture {
    uint256 public count;
    mapping(uint256 => uint256) public entries;
    struct Pair { uint128 left; uint128 right; }
    Pair public pair;
    string public message;
    bytes public data;
    modifier permitted() { require(count < 1000); _; }
    function store(uint256 value) external { count = value; }
    function increment() external { count += 1; }
    function mapped(uint256 key, uint256 value) external { entries[key] = value; }
    function constantMapped(uint256 value) external { entries[7] = value; }
    function setPair(uint128 left, uint128 right) external { pair.left = left; pair.right = right; }
    function setMessage(string calldata value) external { message = value; }
    function setData(bytes calldata value) external { data = value; }
    function helperStore(uint256 value) external permitted { _store(_twice(value)); }
    function _store(uint256 value) private { count = value; }
    function _twice(uint256 value) private pure returns (uint256) { return value * 2; }
    function branch(bytes calldata value) external { if (value.length == 2) count += 1; else count = 5; }
    function negativeZero() external { if (-int256(0) == int256(0)) count += 1; else count = 5; }
    function externalCall(address target) external { (bool success,) = target.call(""); require(success); }
    function opaque(uint256 slot, uint256 value) external { assembly { sstore(slot, value) } }
}
"#;
fn address(byte: u8) -> Address {
    Address::repeat_byte(byte)
}

fn compile() -> Result<Arc<ReproducedArtifact>, Box<dyn std::error::Error>> {
    let path = std::env::var_os("MINUTUS_TEST_OFFICIAL_SOLC_JS")
        .ok_or("set MINUTUS_TEST_OFFICIAL_SOLC_JS to the official pinned solc 0.8.38 JS")?;
    let bytes = std::fs::read(path)?;
    let input = CompilationInput::new(
        "0.8.38+commit.1835d247",
        "Independent.sol",
        "IndependentReviewFixture",
        json!({"language":"Solidity","sources":{"Independent.sol":{"content":SOURCE}},"settings":{"optimizer":{"enabled":true,"runs":200},"viaIR":false,"evmVersion":"cancun","metadata":{"appendCBOR":true,"bytecodeHash":"ipfs"}}}),
    )?;
    let artifact =
        TrustedCompiler::packaged()?.reproduce(&input, &bytes, &AtomicBool::new(false))?;
    Ok(Arc::new(artifact))
}

struct Witness {
    state: ProvedState,
    facts: Vec<(StateQuery, StateValue)>,
    header: Header,
}
type FixtureFacts = Vec<(StateQuery, StateValue)>;
type FixtureSlots = BTreeMap<(Address, U256), U256>;
type FixtureAccounts = BTreeMap<Address, (bool, TrieAccount, Bytes)>;

fn storage_witness() -> Result<(B256, FixtureSlots, FixtureFacts), Box<dyn std::error::Error>> {
    let mut slots = (0..5u64).map(U256::from).collect::<Vec<_>>();
    slots.extend(
        CONTROL_SLOTS
            .iter()
            .map(|slot| U256::from_str(slot))
            .collect::<Result<Vec<_>, _>>()?,
    );
    for key in [7, 9] {
        let mut input = Vec::from(U256::from(key).to_be_bytes::<32>());
        input.extend_from_slice(&U256::ONE.to_be_bytes::<32>());
        slots.push(U256::from_be_bytes(keccak256(input).0));
    }
    for root in [3, 4] {
        let start = U256::from_be_bytes(keccak256(U256::from(root).to_be_bytes::<32>()).0);
        slots.extend([start, start + U256::ONE]);
    }
    slots.push(U256::from(128));
    let paths = slots
        .iter()
        .map(|slot| Nibbles::unpack(keccak256(slot.to_be_bytes::<32>())))
        .collect::<Vec<_>>();
    let mut storage = HashBuilder::default().with_proof_retainer(ProofRetainer::from_iter(paths));
    storage.add_leaf(
        Nibbles::unpack(keccak256(U256::ZERO.to_be_bytes::<32>())),
        &alloy_rlp::encode(U256::from(4)),
    );
    let storage_root = storage.root();
    let storage_nodes = storage.take_proof_nodes();
    let mut values = BTreeMap::new();
    let mut facts = Vec::new();
    for slot in slots {
        let expected = if slot == U256::ZERO {
            U256::from(4)
        } else {
            U256::ZERO
        };
        let key = Nibbles::unpack(keccak256(slot.to_be_bytes::<32>()));
        let nodes = storage_nodes
            .matching_nodes_sorted(&key)
            .into_iter()
            .map(|(_, node)| format!("{node:#x}"))
            .collect::<Vec<_>>();
        let value = json!({"storageProof":[{"key":format!("{slot:#x}"),"value":format!("{expected:#x}"),"proof":nodes}]});
        let checked = proof::storage(storage_root, slot, &value)?;
        values.insert((address(0x22), slot), checked);
        facts.push((
            StateQuery::Storage {
                address: format!("{:#x}", address(0x22)),
                slot: format!("{slot:#066x}"),
            },
            StateValue::Storage {
                value: format!("{checked:#066x}"),
            },
        ));
    }
    Ok((storage_root, values, facts))
}

fn account_witness(
    artifact: &ReproducedArtifact,
    storage_root: B256,
    facts: &mut FixtureFacts,
) -> Result<(B256, FixtureAccounts), Box<dyn std::error::Error>> {
    let account_values = [
        (
            address(0x11),
            TrieAccount {
                nonce: 3,
                balance: U256::from(1_000_000_000_000_000_000u64),
                ..Default::default()
            },
        ),
        (
            address(0x22),
            TrieAccount {
                nonce: 13,
                storage_root,
                code_hash: keccak256(artifact.runtime()),
                ..Default::default()
            },
        ),
    ]
    .into_iter()
    .map(|(a, v)| (keccak256(a), (a, v)))
    .collect::<BTreeMap<_, _>>();
    let targets = [address(0x11), address(0x22), address(0x33)];
    let mut tree = HashBuilder::default().with_proof_retainer(ProofRetainer::from_iter(
        targets.map(|a| Nibbles::unpack(keccak256(a))),
    ));
    for (key, (_, account)) in &account_values {
        tree.add_leaf(Nibbles::unpack(key), &alloy_rlp::encode(*account));
    }
    let root = tree.root();
    let nodes = tree.take_proof_nodes();
    let mut accounts = BTreeMap::new();
    for target in targets {
        let account = account_values
            .get(&keccak256(target))
            .map_or_else(TrieAccount::default, |(_, account)| *account);
        let path = Nibbles::unpack(keccak256(target));
        let proof = nodes
            .matching_nodes_sorted(&path)
            .into_iter()
            .map(|(_, node)| format!("{node:#x}"))
            .collect::<Vec<_>>();
        let value = json!({"address":format!("{target:#x}"),"nonce":format!("{:#x}",account.nonce),"balance":format!("{:#x}",account.balance),"storageHash":format!("{:#x}",account.storage_root),"codeHash":format!("{:#x}",account.code_hash),"accountProof":proof});
        let (exists, checked) = proof::account(root, target, &value)?;
        let code = if target == address(0x22) {
            Bytes::copy_from_slice(artifact.runtime())
        } else {
            Bytes::new()
        };
        assert_eq!(keccak256(&code), checked.code_hash);
        facts.push((
            StateQuery::Account {
                address: format!("{target:#x}"),
            },
            StateValue::Account {
                exists,
                balance: format!("{:#x}", checked.balance),
                nonce: checked.nonce,
                code: format!("{code:#x}"),
            },
        ));
        accounts.insert(target, (exists, checked, code));
    }
    Ok((root, accounts))
}

fn witness(artifact: &ReproducedArtifact) -> Result<Witness, Box<dyn std::error::Error>> {
    let (storage_root, values, mut facts) = storage_witness()?;
    let (root, accounts) = account_witness(artifact, storage_root, &mut facts)?;
    let header = Header {
        state_root: root,
        number: 26_000_000,
        timestamp: 1_793_300_000,
        gas_limit: 30_000_000,
        base_fee_per_gas: Some(1),
        excess_blob_gas: Some(10_000_000),
        beneficiary: address(0x33),
        mix_hash: B256::repeat_byte(0x44),
        ..Default::default()
    };
    Ok(Witness {
        state: ProvedState::new(accounts, values),
        facts,
        header,
    })
}

fn candidate(
    artifact: &Arc<ReproducedArtifact>,
    signature: &str,
    args: &[Value],
) -> Result<GenericContractCandidate, Box<dyn std::error::Error>> {
    let witness = witness(artifact)?;
    let artifacts = [(address(0x22), Arc::clone(artifact))];
    let identity = GenericContractIdentity::from_source_state(
        EvmNetwork::EthereumMainnet,
        address(0x22),
        &witness.state,
        &artifacts,
    )?;
    let intent = generic_contract_intent(
        &identity,
        &format!("{:#x}", address(0x11)),
        "0",
        signature,
        args,
    )?;
    let request = SimulationRequest {
        network: SimulationNetwork::EthereumMainnet,
        block: SimulationBlock {
            hash: format!("{:#x}", witness.header.hash_slow()),
            state_root: format!("{:#x}", witness.header.state_root),
            number: witness.header.number,
            timestamp: witness.header.timestamp,
            beneficiary: format!("{:#x}", witness.header.beneficiary),
            gas_limit: witness.header.gas_limit,
            base_fee: 1,
            prevrandao: format!("{:#x}", witness.header.mix_hash),
            excess_blob_gas: 10_000_000,
            slot_number: None,
            block_access_list_hash: None,
        },
        transaction: SimulationTransaction {
            caller: intent.account().into(),
            target: intent.call().target().into(),
            value: "0x0".into(),
            calldata: intent.call().calldata().into(),
            nonce: 3,
            gas_limit: 1_000_000,
            max_fee_per_gas: 2,
            priority_fee_per_gas: 1,
        },
    };
    let cancelled = AtomicBool::new(false);
    let control = SimulationControl::start(&cancelled);
    let prepared_at = Instant::now();
    let execution = replay_frozen(&request, &witness.facts, &control)?;
    let written = execution
        .written_slots()
        .iter()
        .map(|(a, s)| Ok((Address::from_str(a)?, U256::from_str(s)?)))
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
    let replay = TrustedReplay {
        request,
        state: witness.state,
        execution,
        written,
        prepared_at,
        base: None,
        base_fee: None,
    };
    Ok(GenericContractCandidate::from_replay(
        intent, replay, &artifacts,
    )?)
}

#[test]
#[ignore = "requires normal attested compiler helper and embedded unit-test pin section"]
fn actual_official_compiler_mpt_and_trusted_replay_qualify_independent_functions()
-> Result<(), Box<dyn std::error::Error>> {
    let artifact = compile()?;
    let cases = [
        ("store(uint256)", vec![json!("42")]),
        ("increment()", vec![]),
        ("mapped(uint256,uint256)", vec![json!("9"), json!("17")]),
        ("constantMapped(uint256)", vec![json!("17")]),
        ("setPair(uint128,uint128)", vec![json!("7"), json!("9")]),
        ("setMessage(string)", vec![json!("hello")]),
        ("setMessage(string)", vec![json!("a".repeat(40))]),
        ("setData(bytes)", vec![json!("0x010203")]),
        ("helperStore(uint256)", vec![json!("21")]),
        ("branch(bytes)", vec![json!("0x0102")]),
        ("negativeZero()", vec![]),
    ];
    for (signature, args) in cases {
        let candidate = candidate(&artifact, signature, &args)?;
        assert_eq!(candidate.intent().call().signature(), signature);
        assert!(!candidate.envelope().relations().is_empty());
        assert!(!candidate.effects().storage_changes().is_empty());
        if signature == "branch(bytes)" || signature == "negativeZero()" {
            assert_eq!(
                candidate.envelope().relations()[0].operation,
                GenericEffectOperation::Increase
            );
        }
        if signature == "constantMapped(uint256)" {
            assert_eq!(
                candidate.effects().storage_changes()[0].fields[0].path,
                ["entries", "7"]
            );
        }
    }
    Ok(())
}

#[test]
#[ignore = "requires normal attested compiler helper and embedded unit-test pin section"]
fn real_compiled_external_calls_and_opaque_slots_fail_before_authority()
-> Result<(), Box<dyn std::error::Error>> {
    let artifact = compile()?;
    let witness = witness(&artifact)?;
    let identity = GenericContractIdentity::from_source_state(
        EvmNetwork::EthereumMainnet,
        address(0x22),
        &witness.state,
        &[(address(0x22), Arc::clone(&artifact))],
    )?;
    for (signature, args) in [
        (
            "externalCall(address)",
            vec![json!(format!("{:#x}", address(0x33)))],
        ),
        ("opaque(uint256,uint256)", vec![json!("128"), json!("7")]),
    ] {
        let intent = generic_contract_intent(
            &identity,
            &format!("{:#x}", address(0x11)),
            "0",
            signature,
            &args,
        )?;
        assert!(matches!(
            admit_generic_transition(&identity, &intent),
            Err(GenericError::OpaqueEffects)
        ));
    }
    assert!(
        candidate(
            &artifact,
            "opaque(uint256,uint256)",
            &[json!("128"), json!("7")]
        )
        .is_err()
    );
    Ok(())
}

mod calls;
mod contexts;
