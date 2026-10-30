//! Synthetic local fixtures. These do not qualify real contract effects/source.
#![cfg(target_os = "macos")]
use evm_simulation::*;
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
const CALLER: &str = "0x1111111111111111111111111111111111111111";
const TARGET: &str = "0x2222222222222222222222222222222222222222";
const OTHER: &str = "0x3333333333333333333333333333333333333333";
fn request() -> SimulationRequest {
    SimulationRequest {
        network: SimulationNetwork::EthereumMainnet,
        block: SimulationBlock {
            hash: format!("0x{}", "11".repeat(32)),
            state_root: format!("0x{}", "22".repeat(32)),
            number: 26_000_000,
            timestamp: 1_793_300_000,
            beneficiary: OTHER.into(),
            gas_limit: 30_000_000,
            base_fee: 1,
            prevrandao: format!("0x{}", "44".repeat(32)),
            excess_blob_gas: 10_000_000,
            slot_number: None,
            block_access_list_hash: None,
        },
        transaction: SimulationTransaction {
            caller: CALLER.into(),
            target: TARGET.into(),
            value: "0x0".into(),
            calldata: "0x".into(),
            nonce: 3,
            gas_limit: 1_000_000,
            max_fee_per_gas: 2,
            priority_fee_per_gas: 1,
        },
    }
}
struct Source {
    code: String,
    nested_code: String,
    queries: u32,
    fail: bool,
    storage: String,
}
impl SimulationStateSource for Source {
    fn query(
        &mut self,
        query: &StateQuery,
        control: &SimulationControl<'_>,
    ) -> Result<StateValue, SimulationError> {
        control.remaining()?;
        self.queries += 1;
        if self.fail {
            return Err(SimulationError::StateUnavailable);
        }
        match query {
            StateQuery::Account { address } => Ok(StateValue::Account {
                exists: true,
                balance: if address == CALLER {
                    "0x1000000000000000000".into()
                } else {
                    "0x0".into()
                },
                nonce: if address == CALLER { 3 } else { 0 },
                code: if address == TARGET {
                    self.code.clone()
                } else if address == OTHER {
                    self.nested_code.clone()
                } else {
                    "0x".into()
                },
            }),
            StateQuery::Storage { .. } => Ok(StateValue::Storage {
                value: self.storage.clone(),
            }),
            StateQuery::BlockHash { .. } => Err(SimulationError::StateUnavailable),
        }
    }
}
fn source(code: &str) -> Source {
    Source {
        code: code.into(),
        nested_code: "0x".into(),
        queries: 0,
        fail: false,
        storage: format!("0x{}", "00".repeat(32)),
    }
}
fn run(
    request: &SimulationRequest,
    source: &mut Source,
) -> Result<SimulationObservation, SimulationError> {
    let cancelled = AtomicBool::new(false);
    simulate_in_worker(
        Path::new(env!("CARGO_BIN_EXE_minutus-evm-simulator")),
        request,
        source,
        &SimulationControl::start(&cancelled),
    )
}
#[test]
fn real_worker_executes_storage_and_does_not_qualify_arbitrary_effects()
-> Result<(), SimulationError> {
    let observed = run(&request(), &mut source("0x602a600055602a60005260206000f3"))?;
    assert_eq!(observed.status, ExecutionStatus::Succeeded);
    assert_eq!(observed.storage.len(), 1);
    assert!(observed.storage[0].after.ends_with("2a"));
    assert!(observed.output.ends_with("2a"));
    assert!(!observed.effects_qualified);
    assert_eq!(
        observed
            .accounts
            .iter()
            .find(|account| account.address == CALLER)
            .map(|account| account.nonce),
        Some(4)
    );
    Ok(())
}
#[test]
fn reverts_noops_and_false_return_tokens_never_establish_task_success()
-> Result<(), SimulationError> {
    let reverted = run(&request(), &mut source("0x602a60005560006000fd"))?;
    assert_eq!(reverted.status, ExecutionStatus::Reverted);
    assert!(reverted.storage.is_empty());
    assert!(reverted.logs.is_empty());
    for code in ["0x00", "0x600060005260206000f3"] {
        let observed = run(&request(), &mut source(code))?;
        assert_eq!(observed.status, ExecutionStatus::Succeeded);
        assert!(observed.storage.is_empty());
        assert!(!observed.effects_qualified);
    }
    Ok(())
}
#[test]
fn failures_limits_and_unavailable_state_discard_all_partial_results() {
    let mut missing = source("0x00");
    missing.fail = true;
    assert!(matches!(
        run(&request(), &mut missing),
        Err(SimulationError::StateUnavailable)
    ));
    for code in [
        "0x5b5f56",
        "0x60006000f0",
        "0x730000000000000000000000000000000000000000ff",
    ] {
        let observed = run(&request(), &mut source(code));
        assert!(matches!(
            observed,
            Err(SimulationError::Limit | SimulationError::UnsupportedExecution)
        ));
    }
    let mut invalid = request();
    invalid.transaction.nonce = 4;
    assert!(matches!(
        run(&invalid, &mut source("0x00")),
        Err(SimulationError::TransactionRejected)
    ));
}
#[test]
fn reentrant_frames_are_bounded_before_further_dispatch() {
    let code = format!("0x6000600060006000600073{}620f4240f100", &TARGET[2..]);
    let observed = run(&request(), &mut source(&code));
    assert!(matches!(observed, Err(SimulationError::Limit)));
}
#[test]
fn bpo_blob_fee_schedule_changes_are_observed_at_exact_boundaries() -> Result<(), SimulationError> {
    let mut req = request();
    let mut values = Vec::new();
    for timestamp in [1_765_290_070, 1_765_290_071, 1_767_747_671] {
        req.block.timestamp = timestamp;
        let result = run(&req, &mut source("0x4a60005260206000f3"))?;
        assert_eq!(result.status, ExecutionStatus::Succeeded);
        values.push(result.output);
    }
    assert_ne!(values[0], values[1]);
    assert_ne!(values[1], values[2]);
    Ok(())
}
#[test]
fn chain_variant_and_maintenance_gates_do_not_fall_back_to_latest_eth() {
    let mut req = request();
    req.network = SimulationNetwork::EthereumSepolia;
    assert!(matches!(
        run(&req, &mut source("0x00")),
        Err(SimulationError::UnsupportedRules)
    ));
    for network in [
        SimulationNetwork::BaseMainnet,
        SimulationNetwork::BaseSepolia,
    ] {
        req.network = network;
        assert!(matches!(
            run(&req, &mut source("0x00")),
            Err(SimulationError::BaseEnvelopeRequired)
        ));
    }
    req.network = SimulationNetwork::EthereumMainnet;
    req.block.timestamp = 1_793_491_200;
    assert!(matches!(
        run(&req, &mut source("0x00")),
        Err(SimulationError::UnsupportedRules)
    ));
}
#[test]
fn cancellation_during_state_io_is_terminal_and_no_engine_is_detached() {
    struct CancelSource<'a>(&'a AtomicBool);
    impl SimulationStateSource for CancelSource<'_> {
        fn query(
            &mut self,
            _query: &StateQuery,
            control: &SimulationControl<'_>,
        ) -> Result<StateValue, SimulationError> {
            self.0.store(true, Ordering::Release);
            control.remaining()?;
            Err(SimulationError::StateUnavailable)
        }
    }
    let cancelled = AtomicBool::new(false);
    let result = simulate_in_worker(
        Path::new(env!("CARGO_BIN_EXE_minutus-evm-simulator")),
        &request(),
        &mut CancelSource(&cancelled),
        &SimulationControl::start(&cancelled),
    );
    assert!(matches!(result, Err(SimulationError::Cancelled)));
    cancelled.store(false, Ordering::Release);
    assert!(run(&request(), &mut source("0x00")).is_ok());
}

#[test]
fn public_precompile_vectors_use_the_selected_upstream_backends() -> Result<(), SimulationError> {
    let mut req = request();
    req.transaction.target = format!("0x{}01", "00".repeat(19));
    // Public synthetic secp256k1 vector: z=1, R=G (even y), s=r+1.
    // No secret generation, real account, signature authority or wallet is used.
    req.transaction.calldata = format!(
        "0x{}01{}1b79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f8179879be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81799",
        "00".repeat(31),
        "00".repeat(31)
    );
    let result = run(&req, &mut source("0x"))?;
    assert_eq!(result.status, ExecutionStatus::Succeeded);
    assert_eq!(
        result.output,
        "0x0000000000000000000000007e5f4552091a69125d5dfcb7b8c2659029395bdf"
    );
    req.transaction.target = format!("0x{}05", "00".repeat(19));
    // Three one-byte lengths, then 3^5 mod 7 = 5.
    req.transaction.calldata = format!(
        "0x{}01{}01{}01030507",
        "00".repeat(31),
        "00".repeat(31),
        "00".repeat(31)
    );
    let result = run(&req, &mut source("0x"))?;
    assert_eq!(result.status, ExecutionStatus::Succeeded);
    assert_eq!(result.output, "0x05");
    assert!(!result.effects_qualified);
    Ok(())
}

#[test]
fn logs_and_storage_do_not_establish_token_or_operator_semantics() -> Result<(), SimulationError> {
    let observed = run(&request(), &mut source("0x6001600055600160006000a100"))?;
    assert_eq!(observed.status, ExecutionStatus::Succeeded);
    assert_eq!(observed.logs.len(), 1);
    assert_eq!(observed.storage.len(), 1);
    assert!(!observed.effects_qualified);
    let reverted = run(
        &request(),
        &mut source("0x6001600055600160006000a160006000fd"),
    )?;
    assert_eq!(reverted.status, ExecutionStatus::Reverted);
    assert!(reverted.logs.is_empty());
    assert!(reverted.storage.is_empty());
    Ok(())
}

fn amsterdam() -> SimulationRequest {
    let mut req = request();
    req.network = SimulationNetwork::EthereumSepolia;
    req.block.timestamp = 1_791_294_816;
    req.block.slot_number = Some(42);
    req.block.block_access_list_hash = Some(format!("0x{}", "55".repeat(32)));
    req
}

#[test]
fn amsterdam_requires_exact_consensus_fields_and_exposes_slotnum() -> Result<(), SimulationError> {
    let mut req = amsterdam();
    let code = "0x4b60005260206000f3";
    let observed = run(&req, &mut source(code))?;
    assert_eq!(observed.status, ExecutionStatus::Succeeded);
    assert_eq!(observed.engine, "revm43.0.3/Amsterdam");
    assert_eq!(observed.output, format!("0x{:064x}", 42));
    assert!(!observed.effects_qualified);
    req.block.slot_number = Some(43);
    let changed = run(&req, &mut source(code))?;
    assert_eq!(changed.output, format!("0x{:064x}", 43));
    assert_ne!(observed.request_commitment, changed.request_commitment);
    req.block.slot_number = None;
    assert!(matches!(
        run(&req, &mut source(code)),
        Err(SimulationError::UnsupportedRules)
    ));
    req.block.slot_number = Some(42);
    req.block.block_access_list_hash = None;
    assert!(matches!(
        run(&req, &mut source(code)),
        Err(SimulationError::UnsupportedRules)
    ));
    req = amsterdam();
    req.block.timestamp -= 1;
    assert!(matches!(
        run(&req, &mut source(code)),
        Err(SimulationError::UnsupportedRules)
    ));
    req.block.slot_number = None;
    req.block.block_access_list_hash = None;
    let before = run(&req, &mut source(code))?;
    assert_eq!(before.status, ExecutionStatus::Halted);
    assert_eq!(before.engine, "revm43.0.3/Osaka");
    Ok(())
}

#[test]
fn amsterdam_gas_components_cover_storage_floor_refund_and_native_value()
-> Result<(), SimulationError> {
    let mut req = amsterdam();
    let created = run(&req, &mut source("0x600160005500"))?;
    assert_eq!(created.status, ExecutionStatus::Succeeded);
    assert_eq!(created.state_gas_used, 97_920);
    assert!(created.logs.is_empty());
    assert_eq!(
        created.regular_gas_used + created.state_gas_used,
        created.gas_spent_before_refund
    );
    let mut existing = source("0x600060005500");
    existing.storage = format!("0x{:064x}", 1);
    let clear = run(&req, &mut existing)?;
    assert_eq!(clear.status, ExecutionStatus::Succeeded);
    assert_eq!(clear.state_gas_used, 0);
    assert!(clear.effective_gas_refund > 0);
    assert_eq!(
        clear.gas_used,
        clear.gas_spent_before_refund - clear.effective_gas_refund
    );
    let once = run(&req, &mut source("0x6000545000"))?;
    let twice = run(&req, &mut source("0x600054506000545000"))?;
    assert_eq!(twice.gas_used - once.gas_used, 105);
    req.transaction.calldata = format!("0x{}", "ff".repeat(1024));
    let floor = run(&req, &mut source("0x00"))?;
    assert_eq!(floor.status, ExecutionStatus::Succeeded);
    // Pinned Geth intrinsicBaseGasEIP2780/FloorDataGas: 12,000 base +
    // 3,000 recipient charge (target != caller) + 1,024 * 4 * 16 calldata.
    assert_eq!(floor.floor_gas, 80_536);
    assert_eq!(floor.gas_used, floor.floor_gas);
    req.transaction.target = CALLER.into();
    let self_call = run(&req, &mut source("0x00"))?;
    assert_eq!(self_call.floor_gas, 77_536);
    req.transaction.target = TARGET.into();
    req.transaction.calldata = "0x".into();
    req.transaction.value = "0x1".into();
    let sent = run(&req, &mut source("0x00"))?;
    assert_eq!(sent.status, ExecutionStatus::Succeeded);
    let account = |address: &str| {
        sent.accounts
            .iter()
            .find(|value| value.address == address)
            .ok_or(SimulationError::StateUnavailable)
    };
    let quantity = |value: &str| {
        u128::from_str_radix(value.trim_start_matches("0x"), 16)
            .map_err(|_| SimulationError::InvalidInput)
    };
    let caller = account(CALLER)?;
    assert_eq!(
        quantity(&caller.balance_before)? - quantity(&caller.balance)?,
        1 + u128::from(sent.gas_used) * 2
    );
    assert_eq!(quantity(&account(TARGET)?.balance)?, 1);
    assert_eq!(
        quantity(&account(OTHER)?.balance)?,
        u128::from(sent.gas_used)
    );
    assert_eq!(sent.logs.len(), 1);
    assert_eq!(
        sent.logs[0].address,
        "0xfffffffffffffffffffffffffffffffffffffffe"
    );
    assert_eq!(
        sent.logs[0].topics,
        vec![
            "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef".into(),
            format!("0x{}{}", "00".repeat(12), &CALLER[2..]),
            format!("0x{}{}", "00".repeat(12), &TARGET[2..]),
        ]
    );
    assert_eq!(sent.logs[0].data, format!("0x{:064x}", 1));
    let reverted = run(&req, &mut source("0x60006000fd"))?;
    assert_eq!(reverted.status, ExecutionStatus::Reverted);
    assert!(reverted.logs.is_empty());
    assert!(!sent.effects_qualified);
    Ok(())
}
