use super::*;
use std::{cell::RefCell, collections::VecDeque};

type ExpectedRpc = (&'static str, Value, Result<Value, ContractError>);
struct Fixture(RefCell<VecDeque<ExpectedRpc>>);
impl ReadRpc for Fixture {
    fn value(&self, method: &str, params: Value) -> Result<Value, ContractError> {
        let (expected, arguments, result) = self
            .0
            .borrow_mut()
            .pop_front()
            .ok_or(ContractError::MalformedData)?;
        assert_eq!(method, expected);
        assert_eq!(params, arguments);
        result
    }
    fn call(&self, params: Value) -> Result<RpcCallOutcome, ContractError> {
        let value = self.value("eth_call", params)?;
        if value == json!("revert") {
            return Ok(RpcCallOutcome::Reverted {
                code: 3,
                data: Some("0x12345678".into()),
            });
        }
        Ok(RpcCallOutcome::Succeeded(
            value.as_str().ok_or(ContractError::MalformedData)?.into(),
        ))
    }
}
fn hash() -> String {
    format!("0x{}", "11".repeat(32))
}
fn header() -> Value {
    json!({"hash":hash(),"number":"0x1"})
}
fn fixture(caller: &str, outcome: &str, reorg: bool) -> Fixture {
    let selector = json!({"blockHash":hash(),"requireCanonical":true});
    Fixture(RefCell::new(VecDeque::from([
        ("eth_chainId", json!([]), Ok(json!("0x1"))),
        (
            "eth_getBlockByNumber",
            json!(["latest", false]),
            Ok(header()),
        ),
        (
            "eth_getCode",
            json!([TARGET, selector]),
            Ok(json!("0x6000")),
        ),
        (
            "eth_call",
            json!([{"from":caller,"to":TARGET,"data":"0x26121ff0","gas":"0xf4240","value":"0x0"},selector]),
            Ok(json!(outcome)),
        ),
        (
            "eth_getBlockByNumber",
            json!(["0x1", false]),
            Ok(if reorg {
                json!({"hash":format!("0x{}","22".repeat(32)),"number":"0x1"})
            } else {
                header()
            }),
        ),
    ])))
}
const TARGET: &str = "0x1111111111111111111111111111111111111111";
const ZERO: &str = "0x0000000000000000000000000000000000000000";

#[test]
fn repeated_latest_calls_and_changed_callers_fetch_fresh_pinned_observations()
-> Result<(), ContractError> {
    let call = ContractCall::new(TARGET, "f()", &[], &[super::super::ContractScalar::Bool])?;
    for (caller, word, expected) in [
        (None, '0', "false"),
        (None, '1', "true"),
        (Some(TARGET), '0', "false"),
    ] {
        let data = format!("0x{}{word}", "0".repeat(63));
        let rpc = fixture(caller.unwrap_or(ZERO), &data, false);
        let result = read_at(
            &rpc,
            EvmNetwork::EthereumMainnet,
            &call,
            caller,
            &ContractBlock::Latest,
        )?;
        assert_eq!(
            result.outcome,
            ContractReadOutcome::Returned {
                values: vec![expected.into()],
                data
            }
        );
        assert_eq!(result.caller, caller.unwrap_or(ZERO));
        assert!(!result.context.writes_available);
        assert!(!result.context.source_qualified);
        assert!(rpc.0.borrow().is_empty());
    }
    Ok(())
}

#[test]
fn reorg_discards_result_and_reverts_are_observations() -> Result<(), ContractError> {
    let call = ContractCall::new(TARGET, "f()", &[], &[])?;
    assert_eq!(
        read_at(
            &fixture(ZERO, "0x", true),
            EvmNetwork::EthereumMainnet,
            &call,
            None,
            &ContractBlock::Latest
        ),
        Err(ContractError::NonCanonicalBlock)
    );
    let observed = read_at(
        &fixture(ZERO, "revert", false),
        EvmNetwork::EthereumMainnet,
        &call,
        None,
        &ContractBlock::Latest,
    )?;
    assert!(matches!(
        observed.outcome,
        ContractReadOutcome::Reverted { code: 3, .. }
    ));
    Ok(())
}

#[test]
fn wrong_chain_missing_archive_and_noncanonical_hash_never_fall_back() {
    let wrong = Fixture(RefCell::new(VecDeque::from([(
        "eth_chainId",
        json!([]),
        Ok(json!("0x2105")),
    )])));
    assert_eq!(
        code_at(
            &wrong,
            EvmNetwork::EthereumMainnet,
            TARGET,
            &ContractBlock::Latest
        ),
        Err(ContractError::WrongChain)
    );
    let missing = Fixture(RefCell::new(VecDeque::from([
        ("eth_chainId", json!([]), Ok(json!("0x1"))),
        (
            "eth_getBlockByHash",
            json!([hash(), false]),
            Err(ContractError::ProviderUnavailable),
        ),
    ])));
    assert_eq!(
        code_at(
            &missing,
            EvmNetwork::EthereumMainnet,
            TARGET,
            &ContractBlock::Hash(hash())
        ),
        Err(ContractError::ProviderUnavailable)
    );
    assert!(missing.0.borrow().is_empty());
    for value in ["pending", "safe", "finalized", "0x1", "0xdeadbeef"] {
        assert!(ContractBlock::new(value).is_err());
    }
    assert_eq!(decode_code("0x"), Err(ContractError::NoCode));
    assert!(decode_code("0x0").is_err());
    assert!(decode_code(&format!("0x{}", "00".repeat(24_577))).is_err());
}

#[test]
fn cancellation_after_code_lookup_prevents_calldata_dispatch() -> Result<(), ContractError> {
    let call = ContractCall::new(TARGET, "f()", &[], &[])?;
    let rpc = fixture(ZERO, "0x", false);
    let admitted = AdmittedRpc {
        rpc: &rpc,
        admit: &|| rpc.0.borrow().len() > 2,
    };
    assert_eq!(
        read_at(
            &admitted,
            EvmNetwork::EthereumMainnet,
            &call,
            None,
            &ContractBlock::Latest
        ),
        Err(ContractError::Cancelled)
    );
    assert_eq!(
        rpc.0.borrow().front().map(|entry| entry.0),
        Some("eth_call")
    );
    Ok(())
}

#[test]
fn every_supported_chain_is_checked_before_code_or_call() {
    for network in EvmNetwork::ALL {
        let rpc = Fixture(RefCell::new(VecDeque::from([
            (
                "eth_chainId",
                json!([]),
                Ok(json!(format!("0x{:x}", network.descriptor().chain_id()))),
            ),
            (
                "eth_getBlockByHash",
                json!([hash(), false]),
                Err(ContractError::NonCanonicalBlock),
            ),
        ])));
        assert_eq!(
            code_at(&rpc, network, TARGET, &ContractBlock::Hash(hash())),
            Err(ContractError::NonCanonicalBlock)
        );
        assert!(rpc.0.borrow().is_empty());
    }
}
