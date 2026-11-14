//! Public d=1,k=1 synthetic envelopes; no wallet, signing API or network.
use super::*;
use crate::*;
use alloy_primitives::{U256, hex};
use std::sync::atomic::AtomicBool;
const CALLER: &str = "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf";
const TARGET: &str = "0x2222222222222222222222222222222222222222";
const L1: &str = "0x4200000000000000000000000000000000000015";
const MAIN: &str = "02f8658221050301028307a1209422222222222222222222222222222222222222228080c080a079be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798a05f8d8eb5ac128d6c1e2c41b140c576e4274bff0b610cc52d698bce05511b6fc8";
const SEPOLIA: &str = "02f86683014a340301028307a1209422222222222222222222222222222222222222228080c001a079be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798a001503c210d95b999ccf83f37ff4ea3459b501357ec017a8d617273b9786ad66a";
fn request(network: SimulationNetwork) -> SimulationRequest {
    SimulationRequest {
        network,
        block: SimulationBlock {
            hash: format!("0x{}", "11".repeat(32)),
            state_root: format!("0x{}", "22".repeat(32)),
            number: 53_000_000,
            timestamp: 1_793_300_000,
            beneficiary: format!("0x{}", "33".repeat(20)),
            gas_limit: 30_000_000,
            base_fee: 1,
            prevrandao: format!("0x{}", "44".repeat(32)),
            excess_blob_gas: 0,
            slot_number: None,
            block_access_list_hash: None,
        },
        transaction: SimulationTransaction {
            caller: CALLER.into(),
            target: TARGET.into(),
            value: "0x0".into(),
            calldata: "0x".into(),
            nonce: 3,
            gas_limit: 500_000,
            max_fee_per_gas: 2,
            priority_fee_per_gas: 1,
        },
    }
}
fn snapshot(network: SimulationNetwork, code: &str) -> Result<FrozenBaseRequest, SimulationError> {
    let mut facts = Vec::new();
    for address in [
        CALLER,
        TARGET,
        L1,
        "0x420000000000000000000000000000000000001a",
        "0x4200000000000000000000000000000000000019",
        "0x420000000000000000000000000000000000001b",
        "0x3333333333333333333333333333333333333333",
    ] {
        facts.push(snapshot::Fact {
            query: StateQuery::Account {
                address: address.into(),
            },
            value: StateValue::Account {
                exists: true,
                balance: if address == CALLER {
                    format!("{:#x}", 1_000_000_000_000_000_000u64)
                } else {
                    "0x0".into()
                },
                nonce: if address == CALLER { 3 } else { 0 },
                code: if address == TARGET {
                    code.into()
                } else {
                    "0x".into()
                },
            },
        });
    }
    for (address, slot, value) in [
        (L1, 1, U256::from(1_000_000_000u64)),
        (L1, 7, U256::from(1_000_000)),
        (
            L1,
            3,
            (U256::from(1_000_000) << 96) | (U256::from(1_000_000) << 64),
        ),
        (
            L1,
            8,
            (U256::from(16) << 96) | (U256::from(10) << 64) | U256::from(1000),
        ),
        (TARGET, 0, U256::ZERO),
    ] {
        facts.push(snapshot::Fact {
            query: StateQuery::Storage {
                address: address.into(),
                slot: format!("0x{slot:064x}"),
            },
            value: StateValue::Storage {
                value: format!("{value:#066x}"),
            },
        });
    }
    FrozenBaseRequest::from_facts(request(network), facts)
}
fn expectation() -> Result<OfflineMaterialExpectation, SimulationError> {
    OfflineMaterialExpectation::new(
        vec![StorageChange {
            address: TARGET.into(),
            slot: format!("0x{:064x}", 0),
            before: format!("0x{:064x}", 0),
            after: format!("0x{:064x}", 42),
        }],
        vec![],
        "0x".into(),
    )
}
#[test]
fn actual_native_fee_settlement_and_certificate_both_forks()
-> Result<(), Box<dyn std::error::Error>> {
    for (network, fixture) in [
        (SimulationNetwork::BaseMainnet, MAIN),
        (SimulationNetwork::BaseSepolia, SEPOLIA),
    ] {
        let snapshot = snapshot(network, "0x602a60005500")?;
        let envelope = envelope::Envelope::decode(&snapshot, &hex::decode(fixture)?)?;
        let observation = super::worker::execute(&snapshot, &envelope)?;
        assert_eq!(observation.gas_used, 43_106);
        assert_eq!(observation.storage.len(), 1);
        let ceiling = admission::quantity(&snapshot.fee_ceiling()?.total_wei())?;
        let receipt = OfflineExecution {
            observation,
            envelope,
            snapshot,
        }
        .validate(&expectation()?)?;
        assert!(receipt.transaction_hash().starts_with("0x"));
        assert!(ceiling >= U256::from(1_600_143_193_212u64));
    }
    Ok(())
}
#[test]
fn signed_body_rebinding_and_trailing_bytes_fail_before_worker()
-> Result<(), Box<dyn std::error::Error>> {
    let original = hex::decode(MAIN)?;
    for field in 0..9 {
        let mut snapshot = snapshot(SimulationNetwork::BaseMainnet, "0x00")?;
        match field {
            0 => snapshot.request.transaction.caller = TARGET.into(),
            1 => snapshot.request.transaction.target = CALLER.into(),
            2 => snapshot.request.transaction.nonce += 1,
            3 => snapshot.request.transaction.gas_limit -= 1,
            4 => snapshot.request.transaction.max_fee_per_gas += 1,
            5 => snapshot.request.transaction.priority_fee_per_gas = 0,
            6 => snapshot.request.transaction.value = "0x1".into(),
            7 => snapshot.request.transaction.calldata = "0x01".into(),
            _ => snapshot.request.network = SimulationNetwork::BaseSepolia,
        }
        assert!(envelope::Envelope::decode(&snapshot, &original).is_err());
    }
    let snapshot = snapshot(SimulationNetwork::BaseMainnet, "0x00")?;
    for bytes in [
        vec![],
        vec![0x7e],
        {
            let mut changed = original.clone();
            changed.push(0);
            changed
        },
        {
            let mut changed = original.clone();
            changed[1] = 0xff;
            changed
        },
    ] {
        assert!(envelope::Envelope::decode(&snapshot, &bytes).is_err());
    }
    let cancelled = AtomicBool::new(false);
    let control = SimulationControl::start(&cancelled);
    assert!(matches!(
        transport::execute(
            std::path::Path::new("/does/not/exist"),
            None,
            snapshot,
            &[],
            &control
        ),
        Err(SimulationError::InvalidInput)
    ));
    Ok(())
}
#[test]
fn unknown_state_is_terminal_without_zero_defaults() -> Result<(), Box<dyn std::error::Error>> {
    for (code, missing) in [("0x60015400", 0), ("0x60005500", 1)] {
        let mut snapshot = snapshot(SimulationNetwork::BaseMainnet, code)?;
        if missing == 1 {
            snapshot.facts.retain(|fact|!matches!(&fact.query,StateQuery::Storage{address,slot} if address==L1 && slot.ends_with('3')));
        }
        let envelope = envelope::Envelope::decode(&snapshot, &hex::decode(MAIN)?)?;
        assert!(matches!(
            super::worker::execute(&snapshot, &envelope),
            Err(SimulationError::StateUnavailable)
        ));
    }
    Ok(())
}
#[test]
fn forged_effect_account_and_commitment_are_rejected() -> Result<(), Box<dyn std::error::Error>> {
    for mutation in 0..6 {
        let snapshot = snapshot(SimulationNetwork::BaseMainnet, "0x602a60005500")?;
        let envelope = envelope::Envelope::decode(&snapshot, &hex::decode(MAIN)?)?;
        let mut observation = super::worker::execute(&snapshot, &envelope)?;
        match mutation {
            0 => observation.storage[0].after = format!("0x{:064x}", 43),
            1 => {
                observation
                    .accounts
                    .iter_mut()
                    .find(|account| account.address == CALLER)
                    .ok_or(SimulationError::InconsistentState)?
                    .balance = "0x0".into();
            }
            2 => observation.request_commitment = "00".into(),
            3 => observation.output = "0x42".into(),
            4 => observation.calls.push(observation.calls[0].clone()),
            _ => observation.accounts.clear(),
        }
        assert!(
            OfflineExecution {
                observation,
                envelope,
                snapshot
            }
            .validate(&expectation()?)
            .is_err()
        );
    }
    Ok(())
}
#[test]
fn offline_protocol_cannot_request_state_or_accept_unknown_fields() {
    for message in [
        serde_json::json!({"message":"state","sequence":1,"query":{"kind":"account","address":CALLER}}),
        serde_json::json!({"message":"ready","snapshot_commitment":"x","endpoint":"http://bad"}),
    ] {
        assert!(serde_json::from_value::<transport::WorkerMessage>(message).is_err());
    }
}
#[test]
fn fjord_bound_dominates_actual_native_algorithm_for_adversarial_bytes()
-> Result<(), SimulationError> {
    // Compare against maintained native FastLZ/Fjord, not another compressor.
    use alloy_consensus::{SignableTransaction, TxEip1559};
    let mut seed = 0x1234_5678u32;
    for calldata_length in [0, 1, 31, 32, 55, 56, 127, 128, 255, 256, 1024, 4096, 8192] {
        let mut snapshot = snapshot(SimulationNetwork::BaseMainnet, "0x00")?;
        snapshot.request.transaction.calldata = format!("0x{}", "f1".repeat(calldata_length));
        let ceiling = admission::quantity(&snapshot.fee_ceiling()?.total_wei())?;
        let unsigned = TxEip1559 {
            chain_id: 8453,
            nonce: 3,
            gas_limit: 500_000,
            max_fee_per_gas: 2,
            max_priority_fee_per_gas: 1,
            to: alloy_primitives::TxKind::Call(admission::address(TARGET)?),
            input: alloy_primitives::Bytes::from(vec![0xf1; calldata_length]),
            ..TxEip1559::default()
        };
        let bound = unsigned.encoded_for_signing().len() + 68;
        for pattern in 0..8 {
            let mut bytes: Vec<u8> = (0..bound)
                .map(|index| {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    match pattern {
                        0 => 0,
                        1 => 0xff,
                        2 => index.to_le_bytes()[0] % 32,
                        3 => index.to_le_bytes()[0],
                        4 => {
                            if index % 33 == 0 {
                                seed.to_le_bytes()[0]
                            } else {
                                0
                            }
                        }
                        _ => seed.to_le_bytes()[0],
                    }
                })
                .collect();
            bytes[0] = 2; // ordinary transaction classification, never deposit.
            let mut info = fees::info(&snapshot)?;
            let fee = info.calculate_tx_l1_cost(&bytes, fees::spec(&snapshot)?);
            assert!(
                fee <= ceiling,
                "calldata={calldata_length}, pattern={pattern}"
            );
        }
    }
    Ok(())
}
#[test]
fn high_s_and_overflow_are_rejected_before_worker_launch() -> Result<(), Box<dyn std::error::Error>>
{
    let mut bytes = hex::decode(MAIN)?;
    let length = bytes.len();
    let order = U256::from_be_slice(&hex::decode(
        "fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141",
    )?);
    let high_s = order - U256::from_be_slice(&bytes[length - 32..]);
    bytes[length - 32..].copy_from_slice(&high_s.to_be_bytes::<32>());
    assert!(matches!(
        envelope::Envelope::decode(&snapshot(SimulationNetwork::BaseMainnet, "0x00")?, &bytes),
        Err(SimulationError::InvalidInput)
    ));
    let mut snapshot = snapshot(SimulationNetwork::BaseMainnet, "0x00")?;
    for fact in &mut snapshot.facts {
        if matches!(&fact.query,StateQuery::Storage{address,slot} if address==L1 && slot.ends_with('1'))
        {
            fact.value = StateValue::Storage {
                value: format!("{:#066x}", U256::MAX),
            };
        }
    }
    let cancelled = AtomicBool::new(false);
    assert!(matches!(
        transport::execute(
            std::path::Path::new("/does/not/exist"),
            None,
            snapshot,
            &hex::decode(MAIN)?,
            &SimulationControl::start(&cancelled)
        ),
        Err(SimulationError::Limit)
    ));
    Ok(())
}
#[test]
fn manifest_aliases_contradictions_and_unknown_maps_fail() -> Result<(), SimulationError> {
    use revm_base::Database;
    let snapshot = snapshot(SimulationNetwork::BaseMainnet, "0x00")?;
    let mut facts = snapshot.facts.clone();
    let mut alias = facts[0].clone();
    if let StateQuery::Account { address } = &mut alias.query {
        *address = format!("0x{}", address[2..].to_ascii_uppercase());
    }
    facts.push(alias);
    assert!(matches!(
        FrozenBaseRequest::from_facts(snapshot.request.clone(), facts),
        Err(SimulationError::InvalidInput)
    ));
    let mut facts = snapshot.facts.clone();
    for fact in &mut facts {
        if matches!(&fact.query,StateQuery::Account{address} if address==TARGET) {
            fact.value = StateValue::Account {
                exists: false,
                balance: "0x0".into(),
                nonce: 0,
                code: "0x".into(),
            };
        }
        if matches!(&fact.query,StateQuery::Storage{address,..} if address==TARGET) {
            fact.value = StateValue::Storage {
                value: format!("0x{:064x}", 1),
            };
        }
    }
    assert!(matches!(
        FrozenBaseRequest::from_facts(snapshot.request.clone(), facts),
        Err(SimulationError::InconsistentState)
    ));
    let mut db = database::FrozenDb::new(&snapshot)?;
    assert!(matches!(
        db.basic(alloy_primitives::address!(
            "4444444444444444444444444444444444444444"
        )),
        Err(SimulationError::StateUnavailable)
    ));
    assert!(matches!(
        db.code_by_hash(alloy_primitives::B256::ZERO),
        Err(SimulationError::StateUnavailable)
    ));
    assert!(matches!(
        db.block_hash(52_999_999),
        Err(SimulationError::StateUnavailable)
    ));
    Ok(())
}

#[test]
fn clean_eof_is_distinct_from_extra_or_truncated_frames() -> Result<(), SimulationError> {
    assert!(matches!(
        protocol::read_next_frame::<transport::WorkerMessage>(&mut [].as_slice()),
        Ok(None)
    ));
    for bytes in [vec![0], vec![0, 0, 0, 1], vec![0, 0, 0, 1, b'{']] {
        assert!(
            protocol::read_next_frame::<transport::WorkerMessage>(&mut bytes.as_slice()).is_err()
        );
    }
    let mut bytes = Vec::new();
    protocol::write_frame(
        &transport::WorkerMessage::Ready {
            snapshot_commitment: "known".into(),
        },
        &mut bytes,
    )?;
    let mut input = bytes.as_slice();
    assert!(matches!(
        protocol::read_next_frame::<transport::WorkerMessage>(&mut input),
        Ok(Some(transport::WorkerMessage::Ready { .. }))
    ));
    assert!(matches!(
        protocol::read_next_frame::<transport::WorkerMessage>(&mut input),
        Ok(None)
    ));
    Ok(())
}
