//! Genuine compiler + synthetic MPT + maintained replay cross-contract joins.
//! These fixtures do not exercise signing, a live provider or a native modal.
use super::*;
use evm_simulation::{ReplayCallKind, SimulationStateSource, offline::FrozenBaseRequest};

mod attempted_writes;
mod graph_guards;
mod historical;
mod loops;
mod proxies;
mod witness;

const CALLER_SOURCE: &str = r"// SPDX-License-Identifier: MIT
pragma solidity 0.8.38;
interface ILeaf {
    function store(uint256 x) external;
    function record(uint256 x) external;
    function credit(uint256 x) external;
    function pay(uint256 x) external payable;
}
interface IMiddle { function forward(uint256 x) external; }
contract Caller {
    uint256 public own;
    address public mutableTarget;
    bool public enabled;
    address constant LEAF = 0x4444444444444444444444444444444444444444;
    address constant MIDDLE = 0x5555555555555555555555555555555555555555;
    function route(uint256 x) external { own = x + 1; ILeaf(LEAF).store(x); }
    function routeOnly(uint256 x) external { ILeaf(LEAF).store(x); }
    function routeAdjusted(uint256 x) external { ILeaf(LEAF).store(x + 2); }
    function argumentTarget(address target, uint256 x) external { ILeaf(target).store(x); }
    function routeContext(uint256 x) external { ILeaf(LEAF).record(x); }
    function routeMapping(uint256 x) external { ILeaf(LEAF).credit(x); }
    function chain(uint256 x) external { IMiddle(MIDDLE).forward(x); }
    function guarded(uint256 x) external { require(own < 1000); ILeaf(LEAF).store(x); own = x + 1; }
    function mutableRoute(uint256 x) external { ILeaf(mutableTarget).store(x); }
    function stateBranch(uint256 x) external { if (enabled) ILeaf(LEAF).store(x); own = x; }
    function overwrite(uint256 x) external { ILeaf(LEAF).store(x); ILeaf(LEAF).store(x + 1); }
    function ignoredFailure(uint256 x) external { (bool ok,) = LEAF.call(abi.encodeCall(ILeaf.store, (x))); own = x; }
    function pay(uint256 x) external payable { ILeaf(LEAF).pay{value: 1}(x); }
}
";
const LEAF_SOURCE: &str = r"// SPDX-License-Identifier: MIT
pragma solidity 0.8.38;
contract Leaf {
    uint256 public result;
    address public sender;
    address public origin;
    uint256 public received;
    mapping(address => uint256) public credits;
    function store(uint256 x) external { result = x; }
    function record(uint256 x) external { result = x; sender = msg.sender; origin = tx.origin; }
    function credit(uint256 x) external { credits[msg.sender] = x; }
    function pay(uint256 x) external payable { result = x; received = msg.value; }
}
";
const MIDDLE_SOURCE: &str = r"// SPDX-License-Identifier: MIT
pragma solidity 0.8.38;
interface ILeaf { function store(uint256 x) external; }
contract Middle {
    function forward(uint256 x) external { ILeaf(0x4444444444444444444444444444444444444444).store(x); }
}
";
type Artifacts = Vec<(Address, Arc<ReproducedArtifact>)>;

fn compile_graph() -> Result<Artifacts, Box<dyn std::error::Error>> {
    let path = std::env::var_os("MINUTUS_TEST_OFFICIAL_SOLC_JS").ok_or("official solc required")?;
    let bytes = std::fs::read(path)?;
    let compiler = TrustedCompiler::packaged()?;
    let mut artifacts = Vec::new();
    for (byte, name, source) in [
        (0x22, "Caller", CALLER_SOURCE),
        (0x44, "Leaf", LEAF_SOURCE),
        (0x55, "Middle", MIDDLE_SOURCE),
    ] {
        let file = format!("{name}.sol");
        let input = CompilationInput::new(
            "0.8.38+commit.1835d247",
            &file,
            name,
            json!({"language":"Solidity","sources":{(file.clone()):{"content":source}},"settings":{"optimizer":{"enabled":true,"runs":200},"viaIR":false,"evmVersion":"cancun"}}),
        )?;
        artifacts.push((
            address(byte),
            Arc::new(compiler.reproduce(&input, &bytes, &AtomicBool::new(false))?),
        ));
    }
    Ok(artifacts)
}

fn compile_immutable_graph() -> Result<Artifacts, Box<dyn std::error::Error>> {
    let mut artifacts = compile_graph()?;
    let standard: Value =
        serde_json::from_str(include_str!("calls/fixtures/immutable-caller-input.json"))?;
    let output: Value =
        serde_json::from_str(include_str!("calls/fixtures/immutable-caller-output.json"))?;
    let code =
        &output["contracts"]["ImmutableCaller.sol"]["ImmutableCaller"]["evm"]["deployedBytecode"];
    let mut runtime =
        alloy_primitives::hex::decode(code["object"].as_str().ok_or("missing template")?)?;
    let word = U256::from_be_slice(address(0x44).as_slice()).to_be_bytes::<32>();
    for ranges in code["immutableReferences"]
        .as_object()
        .ok_or("missing references")?
        .values()
    {
        for range in ranges.as_array().ok_or("invalid references")? {
            let start = usize::try_from(range["start"].as_u64().ok_or("missing range")?)?;
            let end = start.checked_add(32).ok_or("range overflow")?;
            runtime
                .get_mut(start..end)
                .ok_or("invalid fixture range")?
                .copy_from_slice(&word);
        }
    }
    // Fixture ranges only construct public synthetic world state. A fresh
    // normal attested compilation independently checks every word/reference;
    // this JSON does not mint a compiler or chain certificate.
    let input =
        CompilationInput::new("0.8.38", "ImmutableCaller.sol", "ImmutableCaller", standard)?;
    let js = std::fs::read(
        std::env::var_os("MINUTUS_TEST_OFFICIAL_SOLC_JS").ok_or("official solc required")?,
    )?;
    let artifact = TrustedCompiler::packaged()?.reproduce_for_runtime(
        &input,
        &js,
        &runtime,
        &AtomicBool::new(false),
    )?;
    assert_eq!(artifact.immutable_bindings().len(), 1);
    artifacts[0].1 = Arc::new(artifact);
    Ok(artifacts)
}

fn scoped_candidate(
    artifacts: &Artifacts,
    network: EvmNetwork,
    function: &str,
    arguments: &[Value],
    value: &str,
    business: &[String],
) -> Result<GenericContractCandidate, Box<dyn std::error::Error>> {
    scoped_candidate_with(
        FixtureScope {
            material: artifacts,
            supplied: artifacts,
            business,
            proxies: &[],
        },
        network,
        function,
        arguments,
        value,
    )
}
#[derive(Clone, Copy)]
struct FixtureScope<'a> {
    material: &'a Artifacts,
    supplied: &'a Artifacts,
    business: &'a [String],
    proxies: &'a [(Address, Address)],
}
fn scoped_candidate_with(
    scope: FixtureScope<'_>,
    network: EvmNetwork,
    function: &str,
    arguments: &[Value],
    value: &str,
) -> Result<GenericContractCandidate, Box<dyn std::error::Error>> {
    let (intent, replay) = scoped_replay(scope, network, function, arguments, value)?;
    Ok(GenericContractCandidate::from_replay_in_scope(
        intent,
        replay,
        scope.supplied,
        scope.business,
    )?)
}
fn scoped_replay(
    scope: FixtureScope<'_>,
    network: EvmNetwork,
    function: &str,
    arguments: &[Value],
    value: &str,
) -> Result<(crate::contract::UnsignedContractIntent, TrustedReplay), Box<dyn std::error::Error>> {
    let witness = witness::build_with_proxies(scope.material, network, scope.proxies)?;
    let identity = GenericContractIdentity::from_source_state(
        network,
        address(0x22),
        &witness.state,
        scope.supplied,
    )?;
    let intent = generic_contract_intent(
        &identity,
        &format!("{:#x}", address(0x11)),
        value,
        function,
        arguments,
    )?;
    let request = witness::request(&witness, &intent)?;
    let cancelled = AtomicBool::new(false);
    let control = SimulationControl::start(&cancelled);
    let prepared_at = Instant::now();
    let execution = replay_frozen(&request, &witness.facts, &control)?;
    let written = execution
        .written_slots()
        .iter()
        .map(|(a, s)| Ok((Address::from_str(a)?, U256::from_str(s)?)))
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
    let (base, base_fee) = if network.is_base() {
        assert!(execution.is_base_application());
        let queries = witness
            .facts
            .iter()
            .map(|(q, _)| q.clone())
            .collect::<Vec<_>>();
        let snapshot = FrozenBaseRequest::prepare(
            request.clone(),
            &queries,
            &mut witness::Source(&witness.facts),
            &control,
        )?;
        let fee = snapshot.fee_ceiling()?.total_wei();
        assert_eq!(execution.fee_ceiling_wei(), Some(fee.as_str()));
        (Some(snapshot), Some(fee))
    } else {
        (None, None)
    };
    let replay = TrustedReplay {
        request,
        state: witness.state,
        execution,
        written,
        prepared_at,
        base,
        base_fee,
    };
    Ok((intent, replay))
}
fn scope() -> Vec<String> {
    [0x22, 0x44, 0x55]
        .map(|byte| format!("{:#x}", address(byte)))
        .to_vec()
}
fn field<'a>(
    candidate: &'a GenericContractCandidate,
    owner: Address,
    name: &str,
) -> Result<&'a Value, Box<dyn std::error::Error>> {
    candidate
        .effects()
        .storage_changes()
        .iter()
        .filter(|word| word.storage_address == format!("{owner:#x}"))
        .flat_map(|word| &word.fields)
        .find(|field| field.path == [name])
        .map(|field| &field.after)
        .ok_or_else(|| "missing independently owned field".into())
}

#[test]
#[ignore = "requires normal attested compiler helper and embedded unit-test pin section"]
fn actual_compiled_cross_contract_graph_preserves_owners_callers_and_calldata()
-> Result<(), Box<dyn std::error::Error>> {
    let artifacts = compile_graph()?;
    for network in [
        EvmNetwork::EthereumMainnet,
        EvmNetwork::EthereumSepolia,
        EvmNetwork::BaseMainnet,
        EvmNetwork::BaseSepolia,
    ] {
        let candidate = scoped_candidate(
            &artifacts,
            network,
            "route(uint256)",
            &[json!("42")],
            "0",
            &scope(),
        )?;
        assert_eq!(field(&candidate, address(0x22), "own")?, &json!("43"));
        assert_eq!(field(&candidate, address(0x44), "result")?, &json!("42"));
        assert_eq!(candidate.replay().calls().len(), 2);
        let frame = &candidate.replay().calls()[1];
        assert_eq!(frame.kind(), ReplayCallKind::Call);
        assert_eq!(frame.target(), format!("{:#x}", address(0x44)));
        assert_eq!(frame.caller(), format!("{:#x}", address(0x22)));
        assert!(frame.succeeded() && frame.committed());
        let expected = crate::contract::DeclaredContractCall::new(
            &format!("{:#x}", address(0x44)),
            "store(uint256)",
            &[json!("42")],
            &[],
        )?;
        assert_eq!(frame.input(), expected.calldata());
        let context = scoped_candidate(
            &artifacts,
            network,
            "routeContext(uint256)",
            &[json!("42")],
            "0",
            &scope(),
        )?;
        assert_eq!(
            field(&context, address(0x44), "sender")?,
            &json!(format!("{:#x}", address(0x22)))
        );
        assert_eq!(
            field(&context, address(0x44), "origin")?,
            &json!(format!("{:#x}", address(0x11)))
        );
        let chain = scoped_candidate(
            &artifacts,
            network,
            "chain(uint256)",
            &[json!("42")],
            "0",
            &scope(),
        )?;
        assert_eq!(chain.replay().calls().len(), 3);
        assert_eq!(field(&chain, address(0x44), "result")?, &json!("42"));
        for function in ["routeOnly(uint256)", "guarded(uint256)"] {
            let candidate =
                scoped_candidate(&artifacts, network, function, &[json!("42")], "0", &scope())?;
            assert_eq!(field(&candidate, address(0x44), "result")?, &json!("42"));
        }
        let adjusted = scoped_candidate(
            &artifacts,
            network,
            "routeAdjusted(uint256)",
            &[json!("42")],
            "0",
            &scope(),
        )?;
        assert_eq!(field(&adjusted, address(0x44), "result")?, &json!("44"));
        let routed = scoped_candidate(
            &artifacts,
            network,
            "argumentTarget(address,uint256)",
            &[json!(format!("{:#x}", address(0x44))), json!("42")],
            "0",
            &scope(),
        )?;
        assert_eq!(field(&routed, address(0x44), "result")?, &json!("42"));
        check_mapping(&artifacts, network)?;
    }
    Ok(())
}

fn check_mapping(
    artifacts: &Artifacts,
    network: EvmNetwork,
) -> Result<(), Box<dyn std::error::Error>> {
    let mapped = scoped_candidate(
        artifacts,
        network,
        "routeMapping(uint256)",
        &[json!("42")],
        "0",
        &scope(),
    )?;
    let mapped_fields = mapped
        .effects()
        .storage_changes()
        .iter()
        .flat_map(|word| &word.fields)
        .collect::<Vec<_>>();
    assert_eq!(mapped_fields.len(), 1);
    assert_eq!(
        mapped_fields[0].path,
        vec!["credits".to_owned(), format!("{:#x}", address(0x22))]
    );
    assert_eq!(mapped_fields[0].after, json!("42"));
    Ok(())
}

#[test]
#[ignore = "requires normal attested compiler helper and embedded unit-test pin section"]
fn actual_compiled_immutable_contract_target_binds_runtime_scope_and_metadata()
-> Result<(), Box<dyn std::error::Error>> {
    let artifacts = compile_immutable_graph()?;
    for network in [
        EvmNetwork::EthereumMainnet,
        EvmNetwork::EthereumSepolia,
        EvmNetwork::BaseMainnet,
        EvmNetwork::BaseSepolia,
    ] {
        let candidate = scoped_candidate(
            &artifacts,
            network,
            "route(uint256)",
            &[json!("42")],
            "0",
            &scope(),
        )?;
        assert_eq!(field(&candidate, address(0x44), "result")?, &json!("42"));
        assert_eq!(candidate.replay().calls().len(), 2);
        let metadata = candidate.identity().source_metadata()?;
        let code = metadata
            .contracts
            .iter()
            .find(|contract| contract.address == format!("{:#x}", address(0x22)))
            .ok_or("missing caller metadata")?;
        let binding = code
            .immutable_bindings
            .iter()
            .find(|binding| binding.name == "target")
            .ok_or("missing immutable metadata")?;
        assert_eq!(binding.solidity_type, "contract ILeaf");
        assert_eq!(binding.canonical_abi_type, "address");
        assert_eq!(binding.value, json!(format!("{:#x}", address(0x44))));
        assert!(binding.ast_id > 0);
    }
    Ok(())
}

#[test]
#[ignore = "requires normal attested compiler helper and embedded unit-test pin section"]
fn actual_compiled_scope_closure_rejects_missing_code_and_state_dependent_expansion()
-> Result<(), Box<dyn std::error::Error>> {
    let artifacts = compile_graph()?;
    for function in [
        "mutableRoute(uint256)",
        "stateBranch(uint256)",
        "overwrite(uint256)",
        "ignoredFailure(uint256)",
    ] {
        assert!(
            scoped_candidate(
                &artifacts,
                EvmNetwork::EthereumMainnet,
                function,
                &[json!("42")],
                "0",
                &scope()
            )
            .is_err(),
            "{function}"
        );
    }
    let root_only = vec![format!("{:#x}", address(0x22))];
    assert!(
        scoped_candidate(
            &artifacts,
            EvmNetwork::EthereumMainnet,
            "route(uint256)",
            &[json!("42")],
            "0",
            &root_only
        )
        .is_err()
    );
    let root_artifact = artifacts[..1].to_vec();
    assert!(
        scoped_candidate_with(
            FixtureScope {
                material: &artifacts,
                supplied: &root_artifact,
                business: &scope(),
                proxies: &[],
            },
            EvmNetwork::EthereumMainnet,
            "route(uint256)",
            &[json!("42")],
            "0",
        )
        .is_err()
    );
    let mut swapped = artifacts.clone();
    swapped[1].1 = Arc::clone(&artifacts[0].1);
    assert!(
        scoped_candidate_with(
            FixtureScope {
                material: &artifacts,
                supplied: &swapped,
                business: &scope(),
                proxies: &[],
            },
            EvmNetwork::EthereumMainnet,
            "route(uint256)",
            &[json!("42")],
            "0"
        )
        .is_err()
    );
    Ok(())
}
