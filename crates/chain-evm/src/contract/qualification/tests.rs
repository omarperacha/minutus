use super::action::{allowance_slot, balance_slot};
use super::*;
use crate::local_simulation::ProvedSimulation;
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use alloy_trie::TrieAccount;
use evm_simulation::{
    AccountChange, CallObservation, ExecutionStatus, LogObservation, SimulationBlock,
    SimulationNetwork, SimulationObservation, SimulationRequest, SimulationTransaction,
    StorageChange,
};
use std::collections::BTreeMap;

const CALLER: Address = Address::repeat_byte(0x11);
const TARGET: Address = Address::repeat_byte(0x22);
const RECIPIENT: Address = Address::repeat_byte(0x33);
const BENEFICIARY: Address = Address::repeat_byte(0x44);

/// The effect accountant is exercised independently from the source certificate.
/// This constructor exists only under cfg(test), never in the production API.
#[derive(Clone)]
struct Fixture {
    request: SimulationRequest,
    observation: SimulationObservation,
    accounts: BTreeMap<Address, (bool, TrieAccount, Bytes)>,
    slots: BTreeMap<(Address, U256), U256>,
    action: ContractAction,
}

impl Fixture {
    fn transfer() -> Self {
        let hash = format!("{:#x}", B256::repeat_byte(0x77));
        let request = SimulationRequest {
            network: SimulationNetwork::EthereumSepolia,
            block: SimulationBlock {
                hash: hash.clone(),
                state_root: format!("{:#x}", B256::repeat_byte(0x88)),
                number: 42,
                timestamp: 1_780_000_000,
                beneficiary: address(BENEFICIARY),
                gas_limit: 30_000_000,
                base_fee: 2,
                prevrandao: format!("{:#x}", B256::repeat_byte(0x99)),
                excess_blob_gas: 0,
                slot_number: None,
                block_access_list_hash: None,
            },
            transaction: SimulationTransaction {
                caller: address(CALLER),
                target: address(TARGET),
                value: "0x0".into(),
                calldata: "0x".into(),
                nonce: 7,
                gas_limit: 100_000,
                max_fee_per_gas: 10,
                priority_fee_per_gas: 1,
            },
        };
        let observation = SimulationObservation {
            request_commitment: format!("{:#x}", B256::repeat_byte(0xaa)),
            block_hash: hash,
            engine: "test-only sealed execution".into(),
            status: ExecutionStatus::Succeeded,
            gas_used: 21_000,
            regular_gas_used: 21_000,
            state_gas_used: 0,
            gas_spent_before_refund: 21_000,
            effective_gas_refund: 0,
            floor_gas: 21_000,
            output: format!("0x{:064x}", 1),
            calls: vec![CallObservation {
                target: address(TARGET),
                code_address: address(TARGET),
                caller: address(CALLER),
                value: "0x0".into(),
                depth: 1,
            }],
            logs: vec![event("Transfer", CALLER, RECIPIENT, U256::from(7))],
            storage: vec![
                storage(balance_slot(CALLER), U256::from(100), U256::from(93)),
                storage(balance_slot(RECIPIENT), U256::from(20), U256::from(27)),
            ],
            accounts: vec![
                account(CALLER, 1_000_000, 937_000, 7, 8),
                account(BENEFICIARY, 100, 21_100, 0, 0),
            ],
            effects_qualified: false,
        };
        let accounts = proved_accounts();
        let slots = [
            ((TARGET, balance_slot(CALLER)), U256::from(100)),
            ((TARGET, balance_slot(RECIPIENT)), U256::from(20)),
        ]
        .into_iter()
        .collect();
        Self {
            request,
            observation,
            accounts,
            slots,
            action: ContractAction::Transfer {
                from: address(CALLER),
                to: address(RECIPIENT),
                amount: "7".into(),
            },
        }
    }

    fn run(&self) -> ProvedSimulation {
        ProvedSimulation::test_fixture(
            self.request.clone(),
            self.observation.clone(),
            self.accounts.clone(),
            self.slots.clone(),
        )
    }
    fn qualify(&self) -> Result<QualifiedEffects, QualificationError> {
        QualifiedEffects::from_run(&self.run(), self.action.clone())
    }
    fn transfer_from(allowance: U256) -> Self {
        let mut fixture = Self::transfer();
        fixture.action = ContractAction::TransferFrom {
            spender: address(CALLER),
            from: address(CALLER),
            to: address(RECIPIENT),
            amount: "7".into(),
        };
        let slot = allowance_slot(CALLER, CALLER);
        fixture.slots.insert((TARGET, slot), allowance);
        if allowance != U256::MAX && allowance >= U256::from(7) {
            fixture
                .observation
                .storage
                .push(storage(slot, allowance, allowance - U256::from(7)));
        }
        fixture
    }
    fn approval(before: U256, after: U256) -> Self {
        let mut fixture = Self::transfer();
        fixture.action = ContractAction::Approve {
            owner: address(CALLER),
            spender: address(RECIPIENT),
            amount: after.to_string(),
        };
        let slot = allowance_slot(CALLER, RECIPIENT);
        fixture.slots = [((TARGET, slot), before)].into_iter().collect();
        fixture.observation.storage = if before == after {
            vec![]
        } else {
            vec![storage(slot, before, after)]
        };
        fixture.observation.logs = vec![event("Approval", CALLER, RECIPIENT, after)];
        fixture
    }
}

fn proved_accounts() -> BTreeMap<Address, (bool, TrieAccount, Bytes)> {
    let runtime = alloy_primitives::hex::decode(
        include_str!("../../../profiles/erc20-oz-5.7.0/runtime.hex")
            .trim()
            .trim_start_matches("0x"),
    )
    .unwrap_or_default();
    [
        (
            CALLER,
            (
                true,
                TrieAccount {
                    balance: U256::from(1_000_000),
                    nonce: 7,
                    ..Default::default()
                },
                Bytes::new(),
            ),
        ),
        (
            TARGET,
            (
                true,
                TrieAccount {
                    balance: U256::from(9),
                    nonce: 1,
                    ..Default::default()
                },
                Bytes::from(runtime),
            ),
        ),
        (
            BENEFICIARY,
            (
                true,
                TrieAccount {
                    balance: U256::from(100),
                    ..Default::default()
                },
                Bytes::new(),
            ),
        ),
    ]
    .into_iter()
    .collect()
}

fn address(value: Address) -> String {
    format!("{value:#x}")
}
fn word(value: U256) -> String {
    format!("{value:#066x}")
}
fn storage(slot: U256, before: U256, after: U256) -> StorageChange {
    StorageChange {
        address: address(TARGET),
        slot: word(slot),
        before: word(before),
        after: word(after),
    }
}
fn account(
    owner: Address,
    before: u64,
    after: u64,
    nonce_before: u64,
    nonce: u64,
) -> AccountChange {
    AccountChange {
        address: address(owner),
        balance_before: format!("0x{before:x}"),
        balance: format!("0x{after:x}"),
        nonce_before,
        nonce,
    }
}
fn event(kind: &str, from: Address, to: Address, amount: U256) -> LogObservation {
    LogObservation {
        address: address(TARGET),
        topics: vec![
            format!(
                "{:#x}",
                keccak256(format!("{kind}(address,address,uint256)"))
            ),
            word(U256::from_be_slice(from.as_slice())),
            word(U256::from_be_slice(to.as_slice())),
        ],
        data: word(amount),
    }
}

#[test]
fn exact_transfer_accounts_for_raw_balances_and_all_native_fees() -> Result<(), QualificationError>
{
    let effects = Fixture::transfer().qualify()?;
    assert_eq!(effects.balances().len(), 2);
    assert_eq!(effects.balances()[0].before, "100");
    assert_eq!(effects.balances()[0].after, "93");
    assert_eq!(effects.balances()[1].before, "20");
    assert_eq!(effects.balances()[1].after, "27");
    assert_eq!(effects.execution_fee_wei(), "63000");
    assert_eq!(effects.maximum_execution_fee_wei(), "1000000");
    assert!(effects.allowance().is_none());
    Ok(())
}

#[test]
fn self_transfer_is_coalesced_and_still_requires_sufficient_proved_balance()
-> Result<(), QualificationError> {
    let mut fixture = Fixture::transfer();
    fixture.action = ContractAction::Transfer {
        from: address(CALLER),
        to: address(CALLER),
        amount: "7".into(),
    };
    fixture.slots.remove(&(TARGET, balance_slot(RECIPIENT)));
    fixture.observation.storage.clear();
    fixture.observation.logs = vec![event("Transfer", CALLER, CALLER, U256::from(7))];
    let effects = fixture.qualify()?;
    assert_eq!(effects.balances().len(), 1);
    assert_eq!(effects.balances()[0].before, effects.balances()[0].after);
    fixture.slots.remove(&(TARGET, balance_slot(CALLER)));
    assert_eq!(
        fixture.qualify().err(),
        Some(QualificationError::MissingState)
    );
    fixture
        .slots
        .insert((TARGET, balance_slot(CALLER)), U256::from(6));
    assert_eq!(
        fixture.qualify().err(),
        Some(QualificationError::Arithmetic)
    );
    Ok(())
}

#[test]
fn maximum_allowance_remains_explicitly_unlimited_without_a_storage_write()
-> Result<(), QualificationError> {
    let mut fixture = Fixture::transfer_from(U256::MAX);
    let effects = fixture.qualify()?;
    let allowance = effects
        .allowance()
        .ok_or(QualificationError::MissingState)?;
    assert!(allowance.unlimited);
    assert_eq!(allowance.before, U256::MAX.to_string());
    assert_eq!(allowance.before, allowance.after);
    fixture
        .slots
        .remove(&(TARGET, allowance_slot(CALLER, CALLER)));
    assert_eq!(
        fixture.qualify().err(),
        Some(QualificationError::MissingState)
    );
    Ok(())
}

#[test]
fn finite_allowance_requires_exact_reduction_and_rejects_insufficient_authority()
-> Result<(), QualificationError> {
    let effects = Fixture::transfer_from(U256::from(30)).qualify()?;
    let allowance = effects
        .allowance()
        .ok_or(QualificationError::MissingState)?;
    assert!(!allowance.unlimited);
    assert_eq!((&*allowance.before, &*allowance.after), ("30", "23"));
    assert_eq!(
        Fixture::transfer_from(U256::from(6)).qualify().err(),
        Some(QualificationError::Arithmetic)
    );
    let mut missing = Fixture::transfer_from(U256::from(30));
    missing.observation.storage.pop();
    assert_eq!(
        missing.qualify().err(),
        Some(QualificationError::UncoveredEffects)
    );
    Ok(())
}

#[test]
fn zero_amount_still_requires_unchanged_balance_proofs_and_exact_event()
-> Result<(), QualificationError> {
    let mut fixture = Fixture::transfer();
    fixture.action = ContractAction::Transfer {
        from: address(CALLER),
        to: address(RECIPIENT),
        amount: "0".into(),
    };
    fixture.observation.storage.clear();
    fixture.observation.logs = vec![event("Transfer", CALLER, RECIPIENT, U256::ZERO)];
    assert_eq!(fixture.qualify()?.balances().len(), 2);
    fixture.slots.remove(&(TARGET, balance_slot(RECIPIENT)));
    assert_eq!(
        fixture.qualify().err(),
        Some(QualificationError::MissingState)
    );
    Ok(())
}

#[test]
fn zero_first_approval_distinguishes_revocation_new_authority_and_unchanged_zero()
-> Result<(), QualificationError> {
    for (before, after) in [
        (U256::ZERO, U256::from(7)),
        (U256::ZERO, U256::MAX),
        (U256::from(7), U256::ZERO),
        (U256::ZERO, U256::ZERO),
    ] {
        let effects = Fixture::approval(before, after).qualify()?;
        let allowance = effects
            .allowance()
            .ok_or(QualificationError::MissingState)?;
        assert_eq!(allowance.before, before.to_string());
        assert_eq!(allowance.after, after.to_string());
        assert_eq!(allowance.unlimited, after == U256::MAX);
    }
    for after in [7, 8] {
        assert_eq!(
            Fixture::approval(U256::from(7), U256::from(after))
                .qualify()
                .err(),
            Some(QualificationError::ApprovalRace)
        );
    }
    Ok(())
}

#[test]
fn no_changed_diff_entry_cannot_substitute_for_a_required_unchanged_allowance_proof() {
    let mut fixture = Fixture::approval(U256::ZERO, U256::ZERO);
    fixture.slots.clear();
    assert_eq!(
        fixture.qualify().err(),
        Some(QualificationError::MissingState)
    );
}

#[test]
fn every_log_field_count_and_order_is_material() {
    for mutation in 0..8 {
        let mut fixture = Fixture::transfer();
        match mutation {
            0 => fixture.observation.logs.clear(),
            1 => fixture
                .observation
                .logs
                .push(fixture.observation.logs[0].clone()),
            2 => fixture.observation.logs[0].address = address(RECIPIENT),
            3 => fixture.observation.logs[0].topics[0] = word(U256::ZERO),
            4 => fixture.observation.logs[0].topics.swap(1, 2),
            5 => fixture.observation.logs[0].data = word(U256::from(8)),
            6 => fixture.observation.logs[0].topics.push(word(U256::ZERO)),
            _ => fixture.observation.logs[0].data.push_str("00"),
        }
        assert_eq!(
            fixture.qualify().err(),
            Some(QualificationError::UncoveredEffects),
            "log mutation {mutation}"
        );
    }
}

#[test]
fn uncovered_missing_spoofed_and_duplicate_storage_changes_reject() {
    for mutation in 0..9 {
        let mut fixture = Fixture::transfer();
        match mutation {
            0 => fixture.observation.storage.clear(),
            1 => {
                fixture.observation.storage.pop();
            }
            2 => fixture.observation.storage[0].address = address(RECIPIENT),
            3 => fixture.observation.storage[0].slot = word(U256::from(2)),
            4 => fixture.observation.storage[0].before = word(U256::from(99)),
            5 => fixture.observation.storage[0].after = word(U256::from(94)),
            6 => fixture
                .observation
                .storage
                .push(fixture.observation.storage[0].clone()),
            7 => {
                fixture
                    .observation
                    .storage
                    .push(storage(U256::from(3), U256::ZERO, U256::from(1)));
            }
            _ => fixture.observation.storage.push(storage(
                balance_slot(CALLER),
                U256::from(100),
                U256::from(100),
            )),
        }
        assert_eq!(
            fixture.qualify().err(),
            Some(QualificationError::UncoveredEffects),
            "storage mutation {mutation}"
        );
    }
}

#[test]
fn native_account_changes_require_exact_before_after_nonce_and_complete_fee_consumption() {
    for mutation in 0..9 {
        let mut fixture = Fixture::transfer();
        match mutation {
            0 => fixture.observation.accounts.clear(),
            1 => {
                fixture.observation.accounts.pop();
            }
            2 => fixture.observation.accounts[0].balance_before = "0xf4241".into(),
            3 => fixture.observation.accounts[0].balance = "0xf4240".into(),
            4 => fixture.observation.accounts[0].nonce_before = 6,
            5 => fixture.observation.accounts[0].nonce = 9,
            6 => fixture
                .observation
                .accounts
                .push(fixture.observation.accounts[0].clone()),
            7 => fixture
                .observation
                .accounts
                .push(account(RECIPIENT, 0, 1, 0, 0)),
            _ => fixture.observation.accounts[1].balance = "0x5275".into(),
        }
        assert_eq!(
            fixture.qualify().err(),
            Some(QualificationError::UncoveredEffects),
            "account mutation {mutation}"
        );
    }
}

#[test]
fn native_beneficiary_aliases_with_caller_or_target_are_aggregated()
-> Result<(), QualificationError> {
    let mut caller = Fixture::transfer();
    caller.request.block.beneficiary = address(CALLER);
    caller.observation.accounts = vec![account(CALLER, 1_000_000, 958_000, 7, 8)];
    assert_eq!(caller.qualify()?.execution_fee_wei(), "63000");
    let mut target = Fixture::transfer();
    target.request.block.beneficiary = address(TARGET);
    target.observation.accounts[1] = account(TARGET, 9, 21_009, 1, 1);
    assert_eq!(target.qualify()?.execution_fee_wei(), "63000");
    target.observation.accounts[1].balance = "0x9".into();
    assert_eq!(
        target.qualify().err(),
        Some(QualificationError::UncoveredEffects)
    );
    Ok(())
}

#[test]
fn overflow_balance_fee_and_missing_native_state_reject() {
    let mut overflow = Fixture::transfer();
    overflow
        .slots
        .insert((TARGET, balance_slot(RECIPIENT)), U256::MAX);
    assert_eq!(
        overflow.qualify().err(),
        Some(QualificationError::Arithmetic)
    );
    let mut balance = Fixture::transfer();
    balance
        .slots
        .insert((TARGET, balance_slot(CALLER)), U256::from(6));
    assert_eq!(
        balance.qualify().err(),
        Some(QualificationError::Arithmetic)
    );
    let mut fee = Fixture::transfer();
    if let Some(value) = fee.accounts.get_mut(&CALLER) {
        value.1.balance = U256::from(1);
    }
    assert_eq!(fee.qualify().err(), Some(QualificationError::Arithmetic));
    for account in [CALLER, TARGET, BENEFICIARY] {
        let mut missing = Fixture::transfer();
        missing.accounts.remove(&account);
        assert_eq!(
            missing.qualify().err(),
            Some(QualificationError::MissingState)
        );
    }
}

#[test]
fn nested_delegate_failed_false_return_or_snapshot_mismatched_runs_reject() {
    for mutation in 0..11 {
        let mut fixture = Fixture::transfer();
        match mutation {
            0 => fixture
                .observation
                .calls
                .push(fixture.observation.calls[0].clone()),
            1 => fixture.observation.calls[0].code_address = address(RECIPIENT),
            2 => fixture.observation.calls[0].caller = address(RECIPIENT),
            3 => fixture.observation.calls[0].depth = 2,
            4 => fixture.observation.calls[0].value = "0x1".into(),
            5 => fixture.observation.status = ExecutionStatus::Reverted,
            6 => fixture.observation.output = word(U256::ZERO),
            7 => fixture.observation.effects_qualified = true,
            8 => fixture.observation.block_hash = word(U256::ZERO),
            9 => fixture.observation.gas_used = 0,
            _ => fixture.observation.gas_used = 100_001,
        }
        assert!(fixture.qualify().is_err(), "execution mutation {mutation}");
    }
}

#[test]
fn material_certificate_uses_proved_state_and_exact_source_without_execution_fee_claims()
-> Result<(), QualificationError> {
    let mut fixture = Fixture::transfer();
    fixture.observation.status = ExecutionStatus::Halted;
    fixture.observation.output = "0xdead".into();
    fixture.observation.calls.clear();
    fixture.observation.logs.clear();
    fixture.observation.storage.clear();
    fixture.observation.accounts.clear();
    fixture.observation.gas_used = u64::MAX;
    let run = fixture.run();
    let material =
        QualifiedMaterialEffects::from_state(run.state(), TARGET, fixture.action.clone())?;
    assert_eq!(material.balances()[0].after, "93");
    assert_eq!(material.balances()[1].after, "27");
    let _expectation = material.offline_expectation();
    assert!(fixture.qualify().is_err());
    if let Some(target) = fixture.accounts.get_mut(&TARGET) {
        target.2 = Bytes::from_static(&[0]);
    }
    assert_eq!(
        QualifiedMaterialEffects::from_state(fixture.run().state(), TARGET, fixture.action.clone())
            .err(),
        Some(QualificationError::UnsupportedProfile)
    );
    Ok(())
}

#[test]
fn exact_profile_instruction_review_excludes_fee_and_external_state_dependencies()
-> Result<(), QualificationError> {
    let fixture = Fixture::transfer();
    let run = fixture.run();
    let code = run
        .state()
        .account(TARGET)
        .ok_or(QualificationError::MissingState)?
        .code();
    super::profile::verify(code)?;
    let length = code.len();
    let metadata_length = usize::from(u16::from_be_bytes([code[length - 2], code[length - 1]]));
    let executable_length = length
        .checked_sub(metadata_length + 2)
        .ok_or(QualificationError::InvalidArtifact)?;
    assert_eq!(executable_length, 1711);
    // This is static review evidence only. Production identity includes all CBOR
    // bytes; it never strips metadata or admits bytecode by an opcode scan.
    let mut offset = 0;
    while offset < executable_length {
        let opcode = code[offset];
        assert!(
            !matches!(
                opcode,
                0x31 | 0x32 | 0x3a | 0x3b | 0x3c | 0x3f | 0x40
                    ..=0x4a | 0x5a | 0xf0 | 0xf1 | 0xf2 | 0xf4 | 0xf5 | 0xfa | 0xff
            ),
            "unexpected dependency opcode {opcode:#x} at {offset}"
        );
        offset += 1 + if (0x60..=0x7f).contains(&opcode) {
            usize::from(opcode - 0x5f)
        } else {
            0
        };
    }
    assert_eq!(offset, executable_length);
    Ok(())
}

#[test]
fn independent_envelope_binding_rejects_every_intent_request_rebinding()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::contract::{DeclaredContractCall, UnsignedContractIntent};
    let call = DeclaredContractCall::new(
        &address(TARGET),
        "transfer(address,uint256)",
        &[
            serde_json::json!(address(RECIPIENT)),
            serde_json::json!("7"),
        ],
        &["bool".into()],
    )?;
    let mut request = Fixture::transfer().request;
    request.transaction.calldata = call.calldata().into();
    let intent = UnsignedContractIntent::new_declared(
        crate::EvmNetwork::EthereumSepolia,
        &address(CALLER),
        "0",
        call,
    )?;
    let (payload, decoded) = super::envelope::prepare(&intent, &request)?;
    assert_eq!(decoded.data(), intent.call().calldata());
    assert!(!payload.is_empty());
    for mutation in 0..5 {
        let mut changed = request.clone();
        match mutation {
            0 => changed.network = SimulationNetwork::BaseSepolia,
            1 => changed.transaction.caller = address(RECIPIENT),
            2 => changed.transaction.target = address(RECIPIENT),
            3 => changed.transaction.calldata.push_str("00"),
            _ => changed.transaction.value = "0x1".into(),
        }
        assert_eq!(
            super::envelope::prepare(&intent, &changed).err(),
            Some(QualificationError::SnapshotMismatch),
            "envelope mutation {mutation}"
        );
    }
    Ok(())
}
