//! Test-only native Base execution using public synthetic signed envelopes.
//! These fixed key=1, nonce=3 vectors must never be funded or submitted. They
//! qualify upstream integration in memory; the production unsigned worker still
//! rejects `BaseEnvelopeRequired` and never receives a signed/broadcast capability.
use alloy_consensus::{Signed, TxEip1559};
use alloy_primitives::{Address, Bytes, TxKind, U256, address, hex, keccak256};
use base_evm::{
    BaseContext, BaseHaltReason, BaseSpecId, BaseTransaction, BaseTransactionError, BaseUpgrade,
    Builder, DefaultBase,
};
use revm_base::{
    Context, Database, InspectEvm,
    context::{BlockEnv, CfgEnv, TxEnv},
    context_interface::result::{EVMError, ExecutionResult, InvalidTransaction, ResultAndState},
    database::{CacheDB, EmptyDB},
    inspector::NoOpInspector,
    state::{AccountInfo, Bytecode},
};
use std::{convert::Infallible, error::Error};

const CALLER: Address = address!("7e5f4552091a69125d5dfcb7b8c2659029395bdf");
const TARGET: Address = address!("2222222222222222222222222222222222222222");
const L1_BLOCK: Address = address!("4200000000000000000000000000000000000015");
const L1_VAULT: Address = address!("420000000000000000000000000000000000001a");
const BASE_VAULT: Address = address!("4200000000000000000000000000000000000019");
const OPERATOR_VAULT: Address = address!("420000000000000000000000000000000000001b");
const BENEFICIARY: Address = address!("3333333333333333333333333333333333333333");
const REGISTRY: Address = address!("8453000000000000000000000000000000000001");
const BALANCE: u64 = 1_000_000_000_000_000_000;
// Genuine EIP-1559 envelopes generated from public d=1, k=1 with low-s
// normalisation outside the app. Independent Alloy decode/recovery below checks
// full consumption, exact re-encoding, chain and signer for every fixture.
const MAIN_CALL: &str = "02f8658221050301028307a1209422222222222222222222222222222222222222228080c080a079be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798a05f8d8eb5ac128d6c1e2c41b140c576e4274bff0b610cc52d698bce05511b6fc8";
const SEPOLIA_CALL: &str = "02f86683014a340301028307a1209422222222222222222222222222222222222222228080c001a079be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798a001503c210d95b999ccf83f37ff4ea3459b501357ec017a8d617273b9786ad66a";
const MAIN_ADMIN: &str = "02f8698221050301028307a1209484530000000000000000000000000000000000018084f851a440c080a079be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798a0574226ef44192b87613955fe384aec7642a8fe9cabbded92db843028eba1598c";
const SEPOLIA_ADMIN: &str = "02f86a83014a340301028307a1209484530000000000000000000000000000000000018084f851a440c080a079be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798a0739262bfedd2e8bb2cf2a390dfed20549bc77ff667379ca59a1d86a1b4f88527";

type TestDb = CacheDB<EmptyDB>;
type Outcome = ResultAndState<BaseHaltReason>;
fn cases() -> [(BaseUpgrade, u64, &'static str, &'static str, Address); 2] {
    [
        (
            BaseUpgrade::Cobalt,
            8453,
            MAIN_CALL,
            MAIN_ADMIN,
            address!("ce3a3bee7e72e2a24079f3c0cb3b97740ed425a9"),
        ),
        (
            BaseUpgrade::Denim,
            84532,
            SEPOLIA_CALL,
            SEPOLIA_ADMIN,
            address!("5f43072722f59964d886cbb507f6a85ca0759d42"),
        ),
    ]
}
fn transaction(fixture: &str, chain_id: u64) -> Result<BaseTransaction<TxEnv>, Box<dyn Error>> {
    let bytes = hex::decode(fixture)?;
    let mut remaining = bytes.as_slice();
    let signed = Signed::<TxEip1559>::eip2718_decode(&mut remaining)?;
    assert!(remaining.is_empty());
    assert_eq!(signed.recover_signer()?, CALLER);
    assert!(signed.signature().normalize_s().is_none());
    let mut encoded = Vec::new();
    signed.eip2718_encode(&mut encoded);
    assert_eq!(bytes, encoded);
    let tx = signed.tx();
    assert_eq!(tx.chain_id, chain_id);
    assert_eq!(tx.nonce, 3);
    assert_eq!(tx.gas_limit, 500_000);
    assert_eq!(tx.max_fee_per_gas, 2);
    assert_eq!(tx.max_priority_fee_per_gas, 1);
    assert_eq!(tx.value, U256::ZERO);
    assert!(tx.access_list.is_empty());
    let mut value = BaseTransaction::new(TxEnv {
        tx_type: 2,
        caller: CALLER,
        kind: tx.to,
        value: tx.value,
        data: tx.input.clone(),
        nonce: tx.nonce,
        gas_limit: tx.gas_limit,
        gas_price: tx.max_fee_per_gas,
        gas_priority_fee: Some(tx.max_priority_fee_per_gas),
        chain_id: Some(tx.chain_id),
        ..TxEnv::default()
    });
    value.enveloped_tx = Some(bytes.into());
    Ok(value)
}
fn database(code: &str, balance: u64) -> Result<TestDb, Box<dyn Error>> {
    let mut db = TestDb::default();
    db.insert_account_info(
        CALLER,
        AccountInfo {
            balance: U256::from(balance),
            nonce: 3,
            ..AccountInfo::default()
        },
    );
    let code = Bytecode::new_raw_checked(Bytes::from(hex::decode(code)?))?;
    db.insert_account_info(
        TARGET,
        AccountInfo {
            code_hash: code.hash_slow(),
            code: Some(code),
            ..AccountInfo::default()
        },
    );
    db.insert_account_info(L1_BLOCK, AccountInfo::default());
    for (slot, value) in [
        (1, U256::from(1_000_000_000_u64)),
        (7, U256::from(1_000_000_u64)),
        (
            3,
            (U256::from(1_000_000) << 96) | (U256::from(1_000_000) << 64),
        ),
        (
            8,
            (U256::from(16) << 96) | (U256::from(10) << 64) | U256::from(1000),
        ),
    ] {
        db.insert_account_storage(L1_BLOCK, U256::from(slot), value)?;
    }
    Ok(db)
}
fn context<DB: Database>(db: DB, upgrade: BaseUpgrade, chain_id: u64) -> BaseContext<DB> {
    let mut cfg = CfgEnv::new_with_spec(BaseSpecId::new(upgrade));
    cfg.chain_id = chain_id;
    Context::base()
        .with_db(db)
        .with_cfg(cfg)
        .with_block(BlockEnv {
            number: U256::from(53_000_000),
            timestamp: U256::from(1_793_300_000_u64),
            gas_limit: 30_000_000,
            basefee: 1,
            beneficiary: BENEFICIARY,
            ..BlockEnv::default()
        })
}
fn execute(
    db: TestDb,
    upgrade: BaseUpgrade,
    chain_id: u64,
    tx: BaseTransaction<TxEnv>,
    admin: Option<Address>,
) -> Result<Outcome, EVMError<Infallible, BaseTransactionError>> {
    context(db, upgrade, chain_id)
        .build_with_inspector_and_activation_admin_address(NoOpInspector {}, admin)
        .inspect_tx(tx)
}
fn balance(outcome: &Outcome, address: Address) -> U256 {
    outcome
        .state
        .get(&address)
        .map_or(U256::ZERO, |account| account.info.balance)
}
fn assert_fees(outcome: &Outcome, gas: u64) {
    // Short fixture envelopes land at Fjord's minimum100-byte estimated size.
    // 100 * (1e6*1e9*16 + 1e6*1e6)/1e6 = 1,600,100,000,000 wei.
    let l1 = U256::from(1_600_100_000_000_u64);
    // Jovian+ operator fee = scalar10 * multiplier100 * gas + constant1000.
    let operator = U256::from(1000_u64) * U256::from(gas) + U256::from(1000_u64);
    let execution = U256::from(gas) * U256::from(2_u64);
    assert_eq!(outcome.result.tx_gas_used(), gas);
    assert_eq!(balance(outcome, L1_VAULT), l1);
    assert_eq!(balance(outcome, BASE_VAULT), U256::from(gas));
    assert_eq!(balance(outcome, BENEFICIARY), U256::from(gas));
    assert_eq!(balance(outcome, OPERATOR_VAULT), operator);
    assert_eq!(
        balance(outcome, CALLER),
        U256::from(BALANCE) - l1 - operator - execution
    );
    assert_eq!(outcome.state.get(&CALLER).map(|a| a.info.nonce), Some(4));
}
#[test]
fn native_cobalt_and_denim_charge_exact_envelope_fees_and_refund_unused_gas()
-> Result<(), Box<dyn Error>> {
    for (upgrade, chain_id, fixture, _, admin) in cases() {
        let tx = transaction(fixture, chain_id)?;
        assert_eq!(tx.base.kind, TxKind::Call(TARGET));
        let result = execute(
            database("602a60005500", BALANCE)?,
            upgrade,
            chain_id,
            tx,
            Some(admin),
        )?;
        assert!(result.result.is_success());
        assert_fees(&result, 43_106);
        assert_eq!(
            result
                .state
                .get(&TARGET)
                .and_then(|a| a.storage.get(&U256::ZERO))
                .map(revm_base::state::EvmStorageSlot::present_value),
            Some(U256::from(42))
        );
    }
    Ok(())
}
#[test]
fn native_reverts_rollback_storage_but_retain_exact_fees_and_nonce() -> Result<(), Box<dyn Error>> {
    for (upgrade, chain_id, fixture, _, admin) in cases() {
        let result = execute(
            database("602a60005560006000fd", BALANCE)?,
            upgrade,
            chain_id,
            transaction(fixture, chain_id)?,
            Some(admin),
        )?;
        assert!(matches!(result.result, ExecutionResult::Revert { .. }));
        assert_fees(&result, 43_112);
        assert!(
            result
                .state
                .get(&TARGET)
                .is_none_or(|a| a.storage.values().all(|v| !v.is_changed()))
        );
    }
    Ok(())
}
#[test]
fn native_additional_fees_participate_in_balance_validation() -> Result<(), Box<dyn Error>> {
    for (upgrade, chain_id, fixture, _, admin) in cases() {
        // Covers ordinary execution maximum, but cannot cover the proved L1 fee.
        let result = execute(
            database("602a60005500", 1_000_000)?,
            upgrade,
            chain_id,
            transaction(fixture, chain_id)?,
            Some(admin),
        );
        assert!(matches!(
            result,
            Err(EVMError::Transaction(BaseTransactionError::Base(
                InvalidTransaction::LackOfFundForMaxFee { .. }
            )))
        ));
    }
    Ok(())
}
#[test]
fn native_activation_admin_uses_chain_fallback_and_proved_state_override()
-> Result<(), Box<dyn Error>> {
    // Pinned registry uses ERC-7201 namespace root; mapping is first, admin next.
    let initial = U256::from_be_bytes(keccak256("base.activation_registry").0) - U256::ONE;
    let root = U256::from_be_bytes(keccak256(initial.to_be_bytes::<32>()).0)
        & (U256::MAX - U256::from(255));
    let rotated = address!("4444444444444444444444444444444444444444");
    for (upgrade, chain_id, _, fixture, admin) in cases() {
        let tx = transaction(fixture, chain_id)?;
        assert_eq!(tx.base.kind, TxKind::Call(REGISTRY));
        for (configured, stored, expected) in [
            (Some(admin), None, admin),
            (None, None, Address::ZERO),
            (Some(admin), Some(rotated), rotated),
        ] {
            let mut db = database("00", BALANCE)?;
            if let Some(stored) = stored {
                db.insert_account_storage(
                    REGISTRY,
                    root + U256::ONE,
                    U256::from_be_slice(stored.as_slice()),
                )?;
            }
            let result = execute(db, upgrade, chain_id, tx.clone(), configured)?;
            assert!(result.result.is_success());
            assert_eq!(
                result
                    .result
                    .output()
                    .map(|data| Address::from_slice(&data[12..])),
                Some(expected)
            );
        }
    }
    Ok(())
}

#[derive(Debug)]
struct MissingFeeState(TestDb);
#[derive(Debug)]
struct Unavailable;
impl std::fmt::Display for Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("synthetic fee state unavailable")
    }
}
impl Error for Unavailable {}
impl revm_base::database_interface::DBErrorMarker for Unavailable {}
impl Database for MissingFeeState {
    type Error = Unavailable;
    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        self.0.basic(address).map_err(|_| Unavailable)
    }
    fn code_by_hash(&mut self, hash: alloy_primitives::B256) -> Result<Bytecode, Self::Error> {
        self.0.code_by_hash(hash).map_err(|_| Unavailable)
    }
    fn storage(&mut self, address: Address, slot: U256) -> Result<U256, Self::Error> {
        if address == L1_BLOCK && slot == U256::from(3) {
            return Err(Unavailable);
        }
        self.0.storage(address, slot).map_err(|_| Unavailable)
    }
    fn block_hash(&mut self, number: u64) -> Result<alloy_primitives::B256, Self::Error> {
        self.0.block_hash(number).map_err(|_| Unavailable)
    }
}
#[test]
fn unavailable_native_fee_state_is_a_database_failure_without_zero_fallback()
-> Result<(), Box<dyn Error>> {
    for (upgrade, chain_id, fixture, _, admin) in cases() {
        let mut engine = context(
            MissingFeeState(database("602a60005500", BALANCE)?),
            upgrade,
            chain_id,
        )
        .build_with_inspector_and_activation_admin_address(NoOpInspector {}, Some(admin));
        let result = engine.inspect_tx(transaction(fixture, chain_id)?);
        assert!(matches!(result, Err(EVMError::Database(Unavailable))));
    }
    Ok(())
}
