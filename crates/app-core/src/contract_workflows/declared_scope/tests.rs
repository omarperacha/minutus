use super::*;
use app_storage::{StoredDeclaredWorkflowStep, StoredWorkflowContract};
use chain_evm::{EvmNetwork, contract::DeclaredContractCall};
use serde_json::json;

const ACCOUNT: &str = "0x1111111111111111111111111111111111111111";
const ROOT: &str = "0x2222222222222222222222222222222222222222";
const CALLEE: &str = "0x3333333333333333333333333333333333333333";
const IMPLEMENTATION: &str = "0x4444444444444444444444444444444444444444";
const FOREIGN: &str = "0x5555555555555555555555555555555555555555";

fn fixture() -> Result<
    (
        ContractWorkflowDeclaration,
        UnsignedContractIntent,
        ScopeFacts,
    ),
    Box<dyn std::error::Error>,
> {
    let declaration = ContractWorkflowDeclaration::new(StoredWorkflowDeclaration {
        version: 2,
        network: "eip155:11155111".into(),
        account: ACCOUNT.into(),
        contracts: vec![
            StoredWorkflowContract {
                id: "router".into(),
                address: ROOT.into(),
            },
            StoredWorkflowContract {
                id: "callee".into(),
                address: CALLEE.into(),
            },
        ],
        steps: vec![StoredDeclaredWorkflowStep {
            id: "route".into(),
            contract: "router".into(),
            function: "route()".into(),
            arguments: vec![],
            value_wei: "0".into(),
            depends_on: vec![],
            prerequisites: vec![],
            completion: vec![],
        }],
        goals: vec![],
    })?;
    let root = DeclaredContractCall::new(ROOT, "route()", &[], &[])?;
    let child = DeclaredContractCall::new(CALLEE, "store(uint256)", &[json!("7")], &[])?;
    let calls = vec![
        InvocationFacts {
            parent: None,
            storage: ROOT.into(),
            code: IMPLEMENTATION.into(),
            caller: ACCOUNT.into(),
            function: root.signature().into(),
            arguments: root.arguments().to_vec(),
            value: "0".into(),
            calldata: root.calldata().into(),
            is_static: false,
        },
        InvocationFacts {
            parent: Some(0),
            storage: CALLEE.into(),
            code: CALLEE.into(),
            caller: ROOT.into(),
            function: child.signature().into(),
            arguments: child.arguments().to_vec(),
            value: "0".into(),
            calldata: child.calldata().into(),
            is_static: false,
        },
    ];
    let intent =
        UnsignedContractIntent::new_declared(EvmNetwork::EthereumSepolia, ACCOUNT, "0", root)?;
    Ok((
        declaration,
        intent,
        ScopeFacts {
            business: vec![ROOT.into(), CALLEE.into()],
            code: vec![ROOT.into(), CALLEE.into(), IMPLEMENTATION.into()],
            relations: vec![
                (ROOT.into(), IMPLEMENTATION.into()),
                (CALLEE.into(), CALLEE.into()),
            ],
            invocations: calls,
        },
    ))
}

#[test]
fn business_scope_and_proof_derived_code_owner_remain_distinct()
-> Result<(), Box<dyn std::error::Error>> {
    let (declaration, intent, facts) = fixture()?;
    validate_facts(declaration.intent(), &intent, &facts)?;
    for business in [
        vec![ROOT.into()],
        vec![ROOT.into(), CALLEE.into(), IMPLEMENTATION.into()],
        vec![ROOT.into(), CALLEE.into(), CALLEE.into()],
    ] {
        let mut changed = facts.clone();
        changed.business = business;
        assert!(validate_facts(declaration.intent(), &intent, &changed).is_err());
    }
    let mut changed = facts.clone();
    changed.invocations[1].storage = IMPLEMENTATION.into();
    assert!(validate_facts(declaration.intent(), &intent, &changed).is_err());
    let mut changed = facts.clone();
    changed.relations[1].1 = FOREIGN.into();
    assert!(validate_facts(declaration.intent(), &intent, &changed).is_err());
    let mut changed = facts;
    changed.relations[1].1 = IMPLEMENTATION.into();
    assert!(validate_facts(declaration.intent(), &intent, &changed).is_err());
    Ok(())
}

#[test]
fn nested_call_parent_caller_static_and_root_binding_cannot_be_relabelled()
-> Result<(), Box<dyn std::error::Error>> {
    let (declaration, intent, facts) = fixture()?;
    for parent in [None, Some(1), Some(u16::MAX)] {
        let mut changed = facts.clone();
        changed.invocations[1].parent = parent;
        assert!(validate_facts(declaration.intent(), &intent, &changed).is_err());
    }
    let mut changed = facts.clone();
    changed.invocations[1].caller = ACCOUNT.into();
    assert!(validate_facts(declaration.intent(), &intent, &changed).is_err());
    let mut changed = facts.clone();
    changed.invocations[0].calldata = "0x00000000".into();
    assert!(validate_facts(declaration.intent(), &intent, &changed).is_err());
    let mut changed = facts.clone();
    changed.invocations[1].is_static = true;
    changed.invocations[1].value = "1".into();
    assert!(validate_facts(declaration.intent(), &intent, &changed).is_err());
    let mut changed = facts;
    changed.invocations[1].is_static = true;
    let mut nested = changed.invocations[1].clone();
    nested.parent = Some(1);
    nested.caller = CALLEE.into();
    nested.is_static = false;
    changed.invocations.push(nested);
    // is_static records the edge opcode. A zero-value CALL can inherit a
    // read-only context; it must not be relabelled as an explicit STATICCALL.
    validate_facts(declaration.intent(), &intent, &changed)?;
    changed.invocations[2].value = "1".into();
    assert!(validate_facts(declaration.intent(), &intent, &changed).is_err());
    Ok(())
}
