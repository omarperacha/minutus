use super::*;
use crate::{ExecutionStatus, SimulationBlock, SimulationNetwork, SimulationTransaction};
use std::sync::atomic::AtomicBool;

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
fn facts(code: &str) -> Vec<(StateQuery, StateValue)> {
    let mut facts = [CALLER, TARGET, OTHER]
        .into_iter()
        .map(|address| {
            (
                StateQuery::Account {
                    address: address.into(),
                },
                StateValue::Account {
                    exists: true,
                    balance: if address == CALLER {
                        "0x1000000000000"
                    } else {
                        "0x64"
                    }
                    .into(),
                    nonce: if address == CALLER { 3 } else { 0 },
                    code: if address == TARGET { code } else { "0x" }.into(),
                },
            )
        })
        .collect::<Vec<_>>();
    facts.push((
        StateQuery::Storage {
            address: TARGET.into(),
            slot: format!("0x{:064x}", 0),
        },
        StateValue::Storage {
            value: format!("0x{:064x}", 0),
        },
    ));
    facts
}
fn run(
    req: &SimulationRequest,
    facts: &[(StateQuery, StateValue)],
) -> Result<TrustedReplayExecution, SimulationError> {
    replay_frozen(
        req,
        facts,
        &SimulationControl::start(&AtomicBool::new(false)),
    )
}
#[test]
fn restored_storage_and_zero_extended_keccak_remain_visible() -> Result<(), SimulationError> {
    let run = run(&request(), &facts("0x6001600055600060005560206000205000"))?;
    assert_eq!(run.observation().status, ExecutionStatus::Succeeded);
    assert!(run.observation().storage.is_empty());
    assert_eq!(
        run.written_slots(),
        [(TARGET.into(), format!("0x{:064x}", 0))]
    );
    assert_eq!(run.keccak_preimages(), [vec![0; 32]]);
    assert_eq!(run.calls().len(), 1);
    assert!(run.calls()[0].committed());
    Ok(())
}
#[test]
fn nested_real_payments_are_distinct_from_delegate_apparent_value() -> Result<(), SimulationError> {
    let mut req = request();
    req.transaction.value = "0x7".into();
    let call = format!("0x6000600060006000600573{}61fffff15000", &OTHER[2..]);
    let run = run(&req, &facts(&call))?;
    assert_eq!(run.native_transfers().len(), 2);
    assert_eq!(
        (
            run.native_transfers()[0].from(),
            run.native_transfers()[0].to(),
            run.native_transfers()[0].value()
        ),
        (CALLER, TARGET, "0x7")
    );
    assert_eq!(
        (
            run.native_transfers()[1].from(),
            run.native_transfers()[1].to(),
            run.native_transfers()[1].value()
        ),
        (TARGET, OTHER, "0x5")
    );
    let delegate = format!("0x600060006000600073{}61fffff45000", &OTHER[2..]);
    let run = super::tests::run(&req, &facts(&delegate))?;
    assert_eq!(run.calls()[1].kind(), ReplayCallKind::DelegateCall);
    assert_eq!(run.calls()[1].value(), "0x7");
    assert_eq!(run.native_transfers().len(), 1);
    Ok(())
}
#[test]
fn reverted_call_payments_do_not_survive_a_successful_outer_frame() -> Result<(), SimulationError> {
    let call = format!("0x6000600060006000600573{}61fffff15000", &OTHER[2..]);
    let mut input = facts(&call);
    let Some((_, StateValue::Account { code, .. })) = input
        .iter_mut()
        .find(|(query, _)| matches!(query,StateQuery::Account{address} if address==OTHER))
    else {
        return Err(SimulationError::Protocol);
    };
    let recipient = "0x4444444444444444444444444444444444444444";
    *code = format!(
        "0x6000600060006000600173{}61fffff15060006000fd",
        &recipient[2..]
    );
    input.push((
        StateQuery::Account {
            address: recipient.into(),
        },
        StateValue::Account {
            exists: false,
            balance: "0x0".into(),
            nonce: 0,
            code: "0x".into(),
        },
    ));
    let run = run(&request(), &input)?;
    assert_eq!(run.observation().status, ExecutionStatus::Succeeded);
    assert!(!run.calls()[1].succeeded());
    assert!(!run.calls()[1].committed());
    assert!(run.calls()[2].succeeded());
    assert!(!run.calls()[2].committed());
    assert!(run.native_transfers().is_empty());
    Ok(())
}

#[test]
fn real_self_call_value_is_retained_even_when_net_balance_is_unchanged()
-> Result<(), SimulationError> {
    let code = format!(
        "0x3615600657005b6000600060016000600173{}61fffff15000",
        &TARGET[2..]
    );
    let result = run(&request(), &facts(&code))?;
    assert_eq!(result.native_transfers().len(), 1);
    let transfer = &result.native_transfers()[0];
    assert_eq!(
        (transfer.from(), transfer.to(), transfer.value()),
        (TARGET, TARGET, "0x1")
    );
    Ok(())
}
#[test]
fn absent_slot_and_duplicate_fact_fail_instead_of_defaulting() {
    let mut input = facts("0x60005400");
    input.pop();
    assert!(matches!(
        run(&request(), &input),
        Err(SimulationError::StateUnavailable)
    ));
    let mut input = facts("0x00");
    input.push(input[0].clone());
    assert!(matches!(
        run(&request(), &input),
        Err(SimulationError::InvalidInput)
    ));
}
#[test]
fn cancelled_replay_and_unsupported_creation_release_no_certificate() {
    assert!(matches!(
        replay_frozen(
            &request(),
            &facts("0x00"),
            &SimulationControl::start(&AtomicBool::new(true))
        ),
        Err(SimulationError::Cancelled)
    ));
    assert!(matches!(
        run(&request(), &facts("0x600060006000f000")),
        Err(SimulationError::UnsupportedExecution)
    ));
}
#[test]
fn environment_sensitive_opcodes_are_recorded() -> Result<(), SimulationError> {
    let run = run(&request(), &facts("0x3a504750485000"))?;
    assert!(run.sensitive_opcodes().contains(&0x3a));
    assert!(run.sensitive_opcodes().contains(&0x47));
    assert!(run.sensitive_opcodes().contains(&0x48));
    Ok(())
}

#[test]
fn discovered_witness_is_replayed_without_a_source_callback() -> Result<(), SimulationError> {
    struct Source {
        facts: Vec<(StateQuery, StateValue)>,
        queries: usize,
    }
    impl crate::SimulationStateSource for Source {
        fn query(
            &mut self,
            query: &StateQuery,
            control: &SimulationControl<'_>,
        ) -> Result<StateValue, SimulationError> {
            control.remaining()?;
            self.queries += 1;
            self.facts
                .iter()
                .find(|(expected, _)| expected == query)
                .map(|(_, value)| value.clone())
                .ok_or(SimulationError::StateUnavailable)
        }
    }
    let mut source = Source {
        facts: facts("0x602a60005500"),
        queries: 0,
    };
    let req = request();
    let cancel = AtomicBool::new(false);
    let control = SimulationControl::start(&cancel);
    let witness = super::replay_discover(&req, &mut source, &control)?;
    assert_eq!(source.queries, 4);
    let result = super::replay_frozen(&req, &witness, &control)?;
    assert_eq!(source.queries, 4);
    assert_eq!(
        result.observation().storage[0].after,
        format!("0x{:064x}", 42)
    );
    Ok(())
}

#[test]
fn host_replay_rejects_native_precompiles_and_retains_ordinary_low_eoa_payments()
-> Result<(), SimulationError> {
    for recipient in [
        format!("0x{:040x}", 1),
        format!("0x{:040x}", 0xa),
        format!("0x{:040x}", 0x100),
    ] {
        let code = format!("0x6000600060006000600073{}61fffff15000", &recipient[2..]);
        let mut input = facts(&code);
        input.push((
            StateQuery::Account { address: recipient },
            StateValue::Account {
                exists: false,
                balance: "0x0".into(),
                nonce: 0,
                code: "0x".into(),
            },
        ));
        assert!(matches!(
            run(&request(), &input),
            Err(SimulationError::UnsupportedExecution)
        ));
    }
    let recipient = format!("0x{:040x}", 0x1234);
    let code = format!("0x6000600060006000600173{}61fffff15000", &recipient[2..]);
    let mut input = facts(&code);
    input.push((
        StateQuery::Account {
            address: recipient.clone(),
        },
        StateValue::Account {
            exists: false,
            balance: "0x0".into(),
            nonce: 0,
            code: "0x".into(),
        },
    ));
    let replay = run(&request(), &input)?;
    assert_eq!(replay.native_transfers()[0].to(), recipient);
    Ok(())
}
