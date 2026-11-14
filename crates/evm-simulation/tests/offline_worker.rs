//! The fixed signed worker runs with the same mandatory sandbox and watchdog.
#![cfg(target_os = "macos")]
use alloy_primitives::{U256, hex};
use evm_simulation::{offline::*, *};
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
const CALLER: &str = "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf";
const TARGET: &str = "0x2222222222222222222222222222222222222222";
const L1: &str = "0x4200000000000000000000000000000000000015";
const SIGNED: &str = "02f8658221050301028307a1209422222222222222222222222222222222222222228080c080a079be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798a05f8d8eb5ac128d6c1e2c41b140c576e4274bff0b610cc52d698bce05511b6fc8";
struct Source {
    code: &'static str,
    queries: usize,
}
impl SimulationStateSource for Source {
    fn query(
        &mut self,
        query: &StateQuery,
        control: &SimulationControl<'_>,
    ) -> Result<StateValue, SimulationError> {
        control.remaining()?;
        self.queries += 1;
        match query {
            StateQuery::Account { address } => Ok(StateValue::Account {
                exists: true,
                balance: if address == CALLER {
                    "0xde0b6b3a7640000".into()
                } else {
                    "0x0".into()
                },
                nonce: if address == CALLER { 3 } else { 0 },
                code: if address == TARGET {
                    self.code.into()
                } else {
                    "0x".into()
                },
            }),
            StateQuery::Storage { address, slot } => {
                let value = if address == L1 {
                    match admission_slot(slot)? {
                        1 => U256::from(1_000_000_000u64),
                        7 => U256::from(1_000_000),
                        3 => (U256::from(1_000_000) << 96) | (U256::from(1_000_000) << 64),
                        8 => (U256::from(16) << 96) | (U256::from(10) << 64) | U256::from(1000),
                        _ => return Err(SimulationError::StateUnavailable),
                    }
                } else {
                    U256::ZERO
                };
                Ok(StateValue::Storage {
                    value: format!("{value:#066x}"),
                })
            }
            StateQuery::BlockHash { .. } => Err(SimulationError::StateUnavailable),
        }
    }
}
fn admission_slot(slot: &str) -> Result<u64, SimulationError> {
    u64::from_str_radix(slot.trim_start_matches("0x"), 16)
        .map_err(|_| SimulationError::InvalidInput)
}
fn frozen(
    source: &mut Source,
    control: &SimulationControl<'_>,
) -> Result<FrozenBaseRequest, SimulationError> {
    let request = SimulationRequest {
        network: SimulationNetwork::BaseMainnet,
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
    };
    let mut manifest: Vec<StateQuery> = [
        CALLER,
        TARGET,
        L1,
        "0x420000000000000000000000000000000000001a",
        "0x4200000000000000000000000000000000000019",
        "0x420000000000000000000000000000000000001b",
        "0x3333333333333333333333333333333333333333",
    ]
    .into_iter()
    .map(|address| StateQuery::Account {
        address: address.into(),
    })
    .collect();
    for (address, slot) in [(L1, 1), (L1, 3), (L1, 7), (L1, 8), (TARGET, 0)] {
        manifest.push(StateQuery::Storage {
            address: address.into(),
            slot: format!("0x{slot:064x}"),
        });
    }
    FrozenBaseRequest::prepare(request, &manifest, source, control)
}
fn expected() -> Result<OfflineMaterialExpectation, SimulationError> {
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
fn actual_offline_worker_has_no_state_source_after_freeze() -> Result<(), Box<dyn std::error::Error>>
{
    let cancelled = AtomicBool::new(false);
    let control = SimulationControl::start(&cancelled);
    let mut source = Source {
        code: "0x602a60005500",
        queries: 0,
    };
    let snapshot = frozen(&mut source, &control)?;
    let queries = source.queries;
    diagnose_base_offline(
        Path::new(env!("CARGO_BIN_EXE_minutus-base-offline-simulator")),
        snapshot,
        &hex::decode(SIGNED)?,
        &expected()?,
        &control,
    )?;
    assert_eq!(source.queries, queries);
    Ok(())
}
#[test]
fn missing_frozen_storage_and_reverts_are_terminal_without_requery()
-> Result<(), Box<dyn std::error::Error>> {
    for code in ["0x60015400", "0x60006000fd"] {
        let cancelled = AtomicBool::new(false);
        let control = SimulationControl::start(&cancelled);
        let mut source = Source { code, queries: 0 };
        let snapshot = frozen(&mut source, &control)?;
        let queries = source.queries;
        let result = diagnose_base_offline(
            Path::new(env!("CARGO_BIN_EXE_minutus-base-offline-simulator")),
            snapshot,
            &hex::decode(SIGNED)?,
            &expected()?,
            &control,
        );
        assert!(matches!(
            result,
            Err(SimulationError::StateUnavailable | SimulationError::TransactionRejected)
        ));
        assert_eq!(source.queries, queries);
    }
    Ok(())
}
#[test]
fn cancellation_and_wrong_online_worker_never_fetch_state() -> Result<(), Box<dyn std::error::Error>>
{
    for cancel in [false, true] {
        let cancelled = AtomicBool::new(false);
        let control = SimulationControl::start(&cancelled);
        let mut source = Source {
            code: "0x00",
            queries: 0,
        };
        let snapshot = frozen(&mut source, &control)?;
        let queries = source.queries;
        cancelled.store(cancel, Ordering::Release);
        let result = diagnose_base_offline(
            Path::new(env!("CARGO_BIN_EXE_minutus-evm-simulator")),
            snapshot,
            &hex::decode(SIGNED)?,
            &expected()?,
            &control,
        );
        assert!(result.is_err());
        assert_eq!(source.queries, queries);
    }
    Ok(())
}
