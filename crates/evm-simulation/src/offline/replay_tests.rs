//! Public synthetic d=1,k=1/2 signature fixtures only. No wallet, network or
//! signing library is involved; none of these addresses may ever be funded.
use super::{
    FrozenBaseRequest, OfflineExecution, OfflineMaterialExpectation, envelope::Envelope, worker,
};
use crate::*;
use alloy_consensus::{SignableTransaction, Signed, TxEip1559};
use alloy_primitives::{Signature, TxKind, U256, hex};
use std::sync::atomic::AtomicBool;
const CALLER: &str = "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf";
const TARGET: &str = "0x2222222222222222222222222222222222222222";
const OTHER: &str = "0x3333333333333333333333333333333333333333";
type Witness = Vec<(StateQuery, StateValue)>;
pub(super) fn fixture(
    network: SimulationNetwork,
    code: &str,
    other_code: &str,
    input: Vec<u8>,
) -> Result<(SimulationRequest, Witness), Box<dyn std::error::Error>> {
    let request = SimulationRequest {
        network,
        block: SimulationBlock {
            hash: format!("0x{}", "11".repeat(32)),
            state_root: format!("0x{}", "22".repeat(32)),
            number: 53_000_000,
            timestamp: 1_793_300_000,
            beneficiary: OTHER.into(),
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
            calldata: format!("0x{}", hex::encode(input)),
            nonce: 3,
            gas_limit: 500_000,
            max_fee_per_gas: 2,
            priority_fee_per_gas: 1,
        },
    };
    let mut facts = Vec::new();
    for query in super::base_native_manifest(&request)? {
        let value = match &query {
            StateQuery::Account { address } => StateValue::Account {
                exists: true,
                balance: if address == CALLER {
                    format!("{:#x}", 1_000_000_000_000_000_000_u64)
                } else {
                    "0x0".into()
                },
                nonce: if address == CALLER { 3 } else { 0 },
                code: if address == TARGET {
                    code
                } else if address == OTHER {
                    other_code
                } else {
                    "0x"
                }
                .into(),
            },
            StateQuery::Storage { slot, .. } => {
                let value = match U256::from_be_bytes(crate::admission::hash(slot)?.0).to::<u64>() {
                    1 => U256::from(1_000_000_000_u64),
                    7 => U256::from(1_000_000),
                    3 => (U256::from(1_000_000) << 96) | (U256::from(1_000_000) << 64),
                    8 => (U256::from(16) << 96) | (U256::from(10) << 64) | U256::from(1000),
                    _ => return Err("unexpected native fee slot".into()),
                };
                StateValue::Storage {
                    value: format!("{value:#066x}"),
                }
            }
            StateQuery::BlockHash { .. } => return Err("unexpected manifest query".into()),
        };
        facts.push((query, value));
    }
    for address in [TARGET, OTHER] {
        facts.push((
            StateQuery::Storage {
                address: address.into(),
                slot: format!("0x{:064x}", 0),
            },
            StateValue::Storage {
                value: format!("0x{:064x}", 0),
            },
        ));
    }
    facts.push((
        StateQuery::Account {
            address: format!("0x{:040x}", 1),
        },
        StateValue::Account {
            exists: false,
            balance: "0x0".into(),
            nonce: 0,
            code: "0x".into(),
        },
    ));
    Ok((request, facts))
}
pub(super) fn signed(
    request: &SimulationRequest,
    k: u64,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let tx = &request.transaction;
    let transaction = TxEip1559 {
        chain_id: request.network.chain_id(),
        nonce: tx.nonce,
        gas_limit: tx.gas_limit,
        max_fee_per_gas: tx.max_fee_per_gas,
        max_priority_fee_per_gas: tx.priority_fee_per_gas,
        to: TxKind::Call(crate::admission::address(&tx.target)?),
        value: U256::ZERO,
        input: crate::admission::data(&tx.calldata, 8192)?.into(),
        ..TxEip1559::default()
    };
    let order = U256::from_be_bytes(hex!(
        "fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141"
    ));
    let r = U256::from_be_bytes(if k == 1 {
        hex!("79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798")
    } else {
        hex!("c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5")
    });
    let z = U256::from_be_bytes(transaction.signature_hash().0);
    let mut s = z.add_mod(r, order).mul_mod(
        U256::from(k).inv_mod(order).ok_or("bad public nonce")?,
        order,
    );
    let mut parity = false;
    if s > order / U256::from(2) {
        s = order - s;
        parity = true;
    }
    let signed = Signed::new_unhashed(transaction, Signature::new(r, s, parity));
    assert_eq!(signed.recover_signer()?, crate::admission::address(CALLER)?);
    let mut bytes = Vec::new();
    signed.eip2718_encode(&mut bytes);
    Ok(bytes)
}
#[test]
fn application_frames_match_genuine_native_transactions_across_signatures_and_nested_calls()
-> Result<(), Box<dyn std::error::Error>> {
    for (network, timestamp) in [
        (SimulationNetwork::BaseMainnet, 1_793_300_000),
        (SimulationNetwork::BaseMainnet, 1_793_901_600),
        (SimulationNetwork::BaseSepolia, 1_793_300_000),
    ] {
        for input_len in [0, 512] {
            for code in [
                format!("0x6000600060006000600073{}5af150602a60005500", &OTHER[2..]),
                format!(
                    "0x363d3d373d3d3d363d73{}5af43d82803e903d91602b57fd5bf3",
                    &OTHER[2..]
                ),
            ] {
                let (mut request, facts) = fixture(
                    network,
                    &code,
                    "0x602b60005500",
                    (0..input_len)
                        .map(|index| u8::try_from((index * 73 + 11) % 251).unwrap_or_default())
                        .collect(),
                )?;
                request.block.timestamp = timestamp;
                let replay = replay_frozen(
                    &request,
                    &facts,
                    &SimulationControl::start(&AtomicBool::new(false)),
                )?;
                assert!(replay.is_base_application());
                assert!(replay.native_transfers().is_empty());
                let expected = OfflineMaterialExpectation::from_trusted_replay(&replay)?;
                for k in [1, 2] {
                    let snapshot = FrozenBaseRequest::from_replay_facts(&request, &facts)?;
                    let envelope = Envelope::decode(&snapshot, &signed(&request, k)?)?;
                    let observation = worker::execute(&snapshot, &envelope)?;
                    assert_eq!(observation.gas_used, replay.observation().gas_used);
                    OfflineExecution {
                        observation,
                        envelope,
                        snapshot,
                    }
                    .validate(&expected)?;
                }
            }
        }
    }
    Ok(())
}
#[test]
fn application_mode_rejects_balance_reads_precompiles_and_value()
-> Result<(), Box<dyn std::error::Error>> {
    for code in [
        "0x4700".to_owned(),
        format!("0x73{}3100", &CALLER[2..]),
        "0x6000600060006000600060015af100".to_owned(),
        format!("0x6000600060006000600173{}5af100", &OTHER[2..]),
    ] {
        let (request, facts) = fixture(SimulationNetwork::BaseSepolia, &code, "0x00", vec![])?;
        let result = replay_frozen(
            &request,
            &facts,
            &SimulationControl::start(&AtomicBool::new(false)),
        );
        match result {
            Err(error) => assert_eq!(error, SimulationError::UnsupportedExecution, "{code}"),
            Ok(_) => return Err(format!("forbidden application admitted: {code}").into()),
        }
    }
    Ok(())
}

#[test]
fn application_mode_rejects_dynamic_native_lookup_before_dispatch()
-> Result<(), Box<dyn std::error::Error>> {
    for timestamp in [1_793_300_000, 1_793_901_600] {
        for address in [
            "8453000000000000000000000000000000000001",
            "8453000000000000000000000000000000000002",
            "b200000000000000000000000000000000000001",
            "b200000000000000000001000000000000000001",
            "813000000000000000000000000000000000aa02",
        ] {
            let code = format!("0x6000600060006000600073{address}5af100");
            let (mut request, mut facts) =
                fixture(SimulationNetwork::BaseMainnet, &code, "0x00", vec![])?;
            request.block.timestamp = timestamp;
            facts.push((
                StateQuery::Account {
                    address: format!("0x{address}"),
                },
                StateValue::Account {
                    exists: false,
                    balance: "0x0".into(),
                    nonce: 0,
                    code: "0x".into(),
                },
            ));
            // Deliberately no activation/token/registry storage facts. Dispatch
            // would request them; lookup must reject before that can happen.
            match replay_frozen(
                &request,
                &facts,
                &SimulationControl::start(&AtomicBool::new(false)),
            ) {
                Err(error) => assert_eq!(error, SimulationError::UnsupportedExecution, "{address}"),
                Ok(_) => return Err(format!("native dispatch admitted: {address}").into()),
            }
        }
    }
    Ok(())
}
