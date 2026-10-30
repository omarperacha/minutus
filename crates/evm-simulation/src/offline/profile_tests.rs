//! Test-only artifact reference across crates: exercises the maintained Base
//! engine with the exact reviewed application profile, without a Rust dependency
//! from evm-simulation back to chain-evm. No key, signing API or network is used.
use super::{
    OfflineExecution, OfflineMaterialExpectation, envelope,
    snapshot::{Fact, FrozenBaseRequest},
    worker,
};
use crate::{
    LogObservation, SimulationBlock, SimulationNetwork, SimulationRequest, SimulationTransaction,
    StateQuery, StateValue, StorageChange,
};
use alloy_consensus::{Signed, TxEip1559};
use alloy_primitives::{Address, B256, Bytes, Signature, TxKind, U256, address, hex, keccak256};
use std::collections::BTreeMap;

const TARGET: Address = address!("2222222222222222222222222222222222222222");
const OWNER: Address = address!("3333333333333333333333333333333333333333");
const RECIPIENT: Address = address!("4444444444444444444444444444444444444444");
const BENEFICIARY: Address = address!("6666666666666666666666666666666666666666");
const L1: Address = address!("4200000000000000000000000000000000000015");

#[derive(Clone, Copy)]
enum Action {
    Approve,
    Transfer,
    TransferFrom,
    TransferFromMaximum,
    SelfTransfer,
    ZeroTransfer,
}

fn mapping(owner: Address, slot: U256) -> U256 {
    let mut bytes = [0_u8; 64];
    bytes[12..32].copy_from_slice(owner.as_slice());
    bytes[32..].copy_from_slice(&slot.to_be_bytes::<32>());
    U256::from_be_bytes(keccak256(bytes).0)
}
fn allowance(owner: Address, spender: Address) -> U256 {
    mapping(spender, mapping(owner, U256::from(1)))
}
fn word(value: U256) -> String {
    format!("{value:#066x}")
}
fn storage(slot: U256, before: U256, after: U256) -> StorageChange {
    StorageChange {
        address: format!("{TARGET:#x}"),
        slot: word(slot),
        before: word(before),
        after: word(after),
    }
}

fn signed(
    network: SimulationNetwork,
    action: Action,
) -> Result<(Signed<TxEip1559>, Address, Bytes), Box<dyn std::error::Error>> {
    let (function, addresses, amount) = match action {
        Action::Approve => ("approve(address,uint256)", vec![RECIPIENT], 7),
        Action::Transfer | Action::SelfTransfer => {
            ("transfer(address,uint256)", vec![RECIPIENT], 7)
        }
        Action::ZeroTransfer => ("transfer(address,uint256)", vec![RECIPIENT], 0),
        Action::TransferFrom | Action::TransferFromMaximum => (
            "transferFrom(address,address,uint256)",
            vec![OWNER, RECIPIENT],
            7,
        ),
    };
    // A self-transfer target must equal the recovered signer, creating a circular
    // fixture dependency. The self-transfer case instead uses transferFrom with
    // an independently fixed equal owner and recipient and recovered spender.
    let (function, addresses) = if matches!(action, Action::SelfTransfer) {
        ("transferFrom(address,address,uint256)", vec![OWNER, OWNER])
    } else {
        (function, addresses)
    };
    let mut input = keccak256(function).as_slice()[..4].to_vec();
    for address in addresses {
        input.extend(U256::from_be_slice(address.as_slice()).to_be_bytes::<32>());
    }
    input.extend(U256::from(amount).to_be_bytes::<32>());
    let transaction = TxEip1559 {
        chain_id: network.chain_id(),
        nonce: 3,
        gas_limit: 500_000,
        max_fee_per_gas: 2,
        max_priority_fee_per_gas: 1,
        to: TxKind::Call(TARGET),
        input: input.into(),
        ..Default::default()
    };
    // Recoverable public scalars choose a valid synthetic sender without a key.
    let signature = Signature::from_scalars_and_parity(
        B256::from(hex!(
            "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
        )),
        B256::from(U256::ONE.to_be_bytes::<32>()),
        false,
    );
    let signed = Signed::new_unhashed(transaction, signature);
    let caller = signed.recover_signer()?;
    let mut bytes = Vec::new();
    signed.eip2718_encode(&mut bytes);
    Ok((signed, caller, bytes.into()))
}

fn profile_case(
    network: SimulationNetwork,
    action: Action,
) -> Result<(FrozenBaseRequest, Bytes, OfflineMaterialExpectation), Box<dyn std::error::Error>> {
    let (signed, caller, bytes) = signed(network, action)?;
    let request = profile_request(network, &signed, caller);
    let mut facts = native_facts(caller)?;
    let (material, expected) = material_facts(caller, action)?;
    facts.extend(material);
    Ok((
        FrozenBaseRequest::from_facts(request, facts)?,
        bytes,
        expected,
    ))
}
fn profile_request(
    network: SimulationNetwork,
    signed: &Signed<TxEip1559>,
    caller: Address,
) -> SimulationRequest {
    SimulationRequest {
        network,
        block: SimulationBlock {
            hash: word(U256::from(1)),
            state_root: word(U256::from(2)),
            number: 53_000_000,
            timestamp: 1_793_300_000,
            beneficiary: format!("{BENEFICIARY:#x}"),
            gas_limit: 30_000_000,
            base_fee: 1,
            prevrandao: word(U256::from(3)),
            excess_blob_gas: 0,
            slot_number: None,
            block_access_list_hash: None,
        },
        transaction: SimulationTransaction {
            caller: format!("{caller:#x}"),
            target: format!("{TARGET:#x}"),
            value: "0x0".into(),
            calldata: format!("{:#x}", signed.tx().input),
            nonce: 3,
            gas_limit: 500_000,
            max_fee_per_gas: 2,
            priority_fee_per_gas: 1,
        },
    }
}
fn native_facts(caller: Address) -> Result<Vec<Fact>, Box<dyn std::error::Error>> {
    let runtime = include_str!("../../../chain-evm/profiles/erc20-oz-5.7.0/runtime.hex").trim();
    assert_eq!(
        keccak256(hex::decode(runtime.trim_start_matches("0x"))?),
        B256::from(hex!(
            "34ad9dbf5175a1e9da19c8edb9d390a0026ac815cdf159a903a04f53e39fd841"
        ))
    );
    let mut facts = Vec::new();
    for address in [
        caller,
        TARGET,
        L1,
        address!("420000000000000000000000000000000000001a"),
        address!("4200000000000000000000000000000000000019"),
        address!("420000000000000000000000000000000000001b"),
        BENEFICIARY,
    ] {
        facts.push(Fact {
            query: StateQuery::Account {
                address: format!("{address:#x}"),
            },
            value: StateValue::Account {
                exists: true,
                balance: if address == caller {
                    "0xde0b6b3a7640000".into()
                } else {
                    "0x0".into()
                },
                nonce: if address == caller {
                    3
                } else {
                    u64::from(address == TARGET)
                },
                code: if address == TARGET {
                    runtime.into()
                } else {
                    "0x".into()
                },
            },
        });
    }
    for (slot, value) in [
        (1, U256::from(1_000_000_000)),
        (7, U256::from(1_000_000)),
        (
            3,
            (U256::from(1_000_000) << 96) | (U256::from(1_000_000) << 64),
        ),
        (
            8,
            (U256::from(16) << 96) | (U256::from(10) << 64) | U256::from(1000),
        ),
    ] {
        facts.push(Fact {
            query: StateQuery::Storage {
                address: format!("{L1:#x}"),
                slot: word(U256::from(slot)),
            },
            value: StateValue::Storage { value: word(value) },
        });
    }
    Ok(facts)
}
fn material_facts(
    caller: Address,
    action: Action,
) -> Result<(Vec<Fact>, OfflineMaterialExpectation), Box<dyn std::error::Error>> {
    let mut facts = Vec::new();
    let mut slots = BTreeMap::new();
    let mut changes = Vec::new();
    let (event, from, to, amount) = match action {
        Action::Approve => {
            let slot = allowance(caller, RECIPIENT);
            slots.insert(slot, U256::ZERO);
            changes.push(storage(slot, U256::ZERO, U256::from(7)));
            ("Approval", caller, RECIPIENT, U256::from(7))
        }
        Action::Transfer | Action::ZeroTransfer => {
            let amount = if matches!(action, Action::ZeroTransfer) {
                U256::ZERO
            } else {
                U256::from(7)
            };
            slots.insert(mapping(caller, U256::ZERO), U256::from(100));
            slots.insert(mapping(RECIPIENT, U256::ZERO), U256::from(20));
            if !amount.is_zero() {
                changes.push(storage(
                    mapping(caller, U256::ZERO),
                    U256::from(100),
                    U256::from(93),
                ));
                changes.push(storage(
                    mapping(RECIPIENT, U256::ZERO),
                    U256::from(20),
                    U256::from(27),
                ));
            }
            ("Transfer", caller, RECIPIENT, amount)
        }
        Action::TransferFrom | Action::TransferFromMaximum | Action::SelfTransfer => {
            let allowance_value = if matches!(action, Action::TransferFromMaximum) {
                U256::MAX
            } else {
                U256::from(30)
            };
            let slot = allowance(OWNER, caller);
            slots.insert(slot, allowance_value);
            if allowance_value != U256::MAX {
                changes.push(storage(slot, U256::from(30), U256::from(23)));
            }
            slots.insert(mapping(OWNER, U256::ZERO), U256::from(100));
            let to = if matches!(action, Action::SelfTransfer) {
                OWNER
            } else {
                RECIPIENT
            };
            if to != OWNER {
                slots.insert(mapping(to, U256::ZERO), U256::from(20));
                changes.push(storage(
                    mapping(OWNER, U256::ZERO),
                    U256::from(100),
                    U256::from(93),
                ));
                changes.push(storage(
                    mapping(to, U256::ZERO),
                    U256::from(20),
                    U256::from(27),
                ));
            }
            ("Transfer", OWNER, to, U256::from(7))
        }
    };
    for (slot, value) in slots {
        facts.push(Fact {
            query: StateQuery::Storage {
                address: format!("{TARGET:#x}"),
                slot: word(slot),
            },
            value: StateValue::Storage { value: word(value) },
        });
    }
    changes.sort_by(|a, b| a.slot.cmp(&b.slot));
    let logs = vec![LogObservation {
        address: format!("{TARGET:#x}"),
        topics: vec![
            format!(
                "{:#x}",
                keccak256(format!("{event}(address,address,uint256)"))
            ),
            word(U256::from_be_slice(from.as_slice())),
            word(U256::from_be_slice(to.as_slice())),
        ],
        data: word(amount),
    }];
    Ok((
        facts,
        OfflineMaterialExpectation::new(changes, logs, word(U256::ONE))?,
    ))
}

#[test]
fn reviewed_profile_material_matches_actual_cobalt_and_denim_execution()
-> Result<(), Box<dyn std::error::Error>> {
    for network in [
        SimulationNetwork::BaseMainnet,
        SimulationNetwork::BaseSepolia,
    ] {
        for action in [
            Action::Approve,
            Action::Transfer,
            Action::TransferFrom,
            Action::TransferFromMaximum,
            Action::SelfTransfer,
            Action::ZeroTransfer,
        ] {
            let (snapshot, bytes, expected) = profile_case(network, action)?;
            let envelope = envelope::Envelope::decode(&snapshot, &bytes)?;
            let observation = worker::execute(&snapshot, &envelope)?;
            assert!(observation.gas_used <= 500_000);
            let validated = OfflineExecution {
                observation,
                envelope,
                snapshot,
            }
            .validate(&expected)?;
            assert!(validated.transaction_hash().starts_with("0x"));
        }
    }
    Ok(())
}
