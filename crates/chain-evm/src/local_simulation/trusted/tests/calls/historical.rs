//! Genuine old compiler reproduction: eligibility never rewrites deployment settings.
use super::*;

const CALLER: &str = r"// SPDX-License-Identifier: MIT
pragma solidity 0.8.7;
interface ILeaf { function store(uint256 x) external; function credit(uint256 x) external; }
contract HistoricalCaller {
    uint256 public own;
    address public mutableTarget;
    bool public enabled;
    modifier bounded() { require(own < 1000, 'bounded'); _; }
    function increment(uint256 x) external { own += x; }
    function decrease(uint256 x) external { own -= x; }
    function helperStore(uint256 x) external bounded { own = twice(x); }
    function twice(uint256 x) private pure returns (uint256) { return x * 2; }
    function route(uint256 x) external { own = x + 1; ILeaf(0x4444444444444444444444444444444444444444).store(x); }
    function routeMapping(uint256 x) external { ILeaf(0x4444444444444444444444444444444444444444).credit(x); }
}
";

fn settings() -> Value {
    // viaIR was not supplied to the historical deployment; omission is exact.
    json!({"optimizer":{"enabled":false,"runs":200},"evmVersion":"berlin"})
}
fn compile_historical(
    name: &str,
    source: &str,
    settings: &Value,
) -> Result<Arc<ReproducedArtifact>, Box<dyn std::error::Error>> {
    let js = std::fs::read(
        std::env::var_os("MINUTUS_TEST_HISTORICAL_SOLC_JS")
            .ok_or("official historical solc required")?,
    )?;
    let file = format!("{name}.sol");
    let input = CompilationInput::new(
        "0.8.7+commit.e28d00a7",
        &file,
        name,
        json!({"language":"Solidity","sources":{(file.clone()):{"content":source}},"settings":settings}),
    )?;
    let artifact = TrustedCompiler::packaged()?.reproduce(&input, &js, &AtomicBool::new(false))?;
    if name == "HistoricalCaller"
        && let Some(path) = std::env::var_os("MINUTUS_TEST_HISTORICAL_AST")
    {
        std::fs::write(path, serde_json::to_vec_pretty(artifact.source_ast())?)?;
    }
    Ok(Arc::new(artifact))
}
fn historical_graph() -> Result<Artifacts, Box<dyn std::error::Error>> {
    Ok(vec![
        (
            address(0x22),
            compile_historical("HistoricalCaller", CALLER, &settings())?,
        ),
        (
            address(0x44),
            compile_historical("Leaf", &LEAF_SOURCE.replace("0.8.38", "0.8.7"), &settings())?,
        ),
        (
            address(0x55),
            compile_historical(
                "Middle",
                &MIDDLE_SOURCE.replace("0.8.38", "0.8.7"),
                &settings(),
            )?,
        ),
    ])
}

fn preflight(
    material: &Artifacts,
    network: EvmNetwork,
    entry: Address,
    function: &str,
    arguments: &[Value],
    proxies: &[(Address, Address)],
) -> Result<(), Box<dyn std::error::Error>> {
    let witness = witness::build_with_proxies(material, network, proxies)?;
    let identity =
        GenericContractIdentity::from_source_state(network, entry, &witness.state, material)?;
    let others = material
        .iter()
        .filter(|(owner, _)| *owner != entry)
        .map(|(owner, _)| {
            GenericContractIdentity::from_source_state(network, *owner, &witness.state, material)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let intent = generic_contract_intent(
        &identity,
        &format!("{:#x}", address(0x11)),
        "0",
        function,
        arguments,
    )?;
    crate::contract::generic::admit_generic_transition_in_scope(
        &identity,
        &others.iter().collect::<Vec<_>>(),
        &intent,
    )?;
    Ok(())
}

#[test]
#[ignore = "requires normal attested compiler helper and official historical compiler JS"]
fn actual_historical_unoptimised_scalar_helpers_mappings_and_fixed_calls_match_proved_replay()
-> Result<(), Box<dyn std::error::Error>> {
    let artifacts = historical_graph()?;
    for (_, artifact) in &artifacts {
        assert_eq!(artifact.compiler_version(), "0.8.7+commit.e28d00a7");
        assert_eq!(
            artifact.compilation_settings()["optimizer"]["enabled"],
            false
        );
        assert!(artifact.compilation_settings().get("viaIR").is_none());
    }
    for network in [
        EvmNetwork::EthereumMainnet,
        EvmNetwork::EthereumSepolia,
        EvmNetwork::BaseMainnet,
        EvmNetwork::BaseSepolia,
    ] {
        for (function, argument, result) in [
            ("increment(uint256)", "3", "8"),
            ("decrease(uint256)", "3", "2"),
            ("helperStore(uint256)", "21", "42"),
        ] {
            preflight(
                &artifacts,
                network,
                address(0x22),
                function,
                &[json!(argument)],
                &[],
            )
            .map_err(|error| format!("static {network:?} {function}: {error}"))?;
            let candidate = scoped_candidate(
                &artifacts,
                network,
                function,
                &[json!(argument)],
                "0",
                &scope(),
            )
            .map_err(|error| format!("candidate {network:?} {function}: {error}"))?;
            assert_eq!(field(&candidate, address(0x22), "own")?, &json!(result));
        }
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
        check_mapping(&artifacts, network)?;
    }
    let maximum = U256::MAX - U256::from(5);
    let candidate = scoped_candidate(
        &artifacts,
        EvmNetwork::EthereumMainnet,
        "increment(uint256)",
        &[json!(maximum.to_string())],
        "0",
        &scope(),
    )?;
    assert_eq!(
        field(&candidate, address(0x22), "own")?,
        &json!(U256::MAX.to_string())
    );
    assert!(
        scoped_candidate(
            &artifacts,
            EvmNetwork::EthereumMainnet,
            "increment(uint256)",
            &[json!((maximum + U256::ONE).to_string())],
            "0",
            &scope()
        )
        .is_err()
    );
    assert!(
        scoped_candidate(
            &artifacts,
            EvmNetwork::EthereumMainnet,
            "decrease(uint256)",
            &[json!("6")],
            "0",
            &scope()
        )
        .is_err()
    );
    Ok(())
}

#[test]
#[ignore = "requires normal attested compiler helper and official historical compiler JS"]
fn actual_historical_original_settings_reject_even_when_metadata_discards_details()
-> Result<(), Box<dyn std::error::Error>> {
    let safe = "pragma solidity 0.8.7; contract Counter { uint public own; function store(uint x) external { own = x; } }";
    let mut explicit_details = settings();
    explicit_details["optimizer"]["details"] = json!({});
    let mut enabled = settings();
    enabled["optimizer"]["enabled"] = json!(true);
    let mut omitted = settings();
    omitted["optimizer"]
        .as_object_mut()
        .ok_or("optimizer missing")?
        .remove("enabled");
    for (configuration, discarded_details) in
        [(explicit_details, true), (enabled, false), (omitted, false)]
    {
        let artifact = compile_historical("Counter", safe, &configuration)?;
        let mut retained = artifact.compilation_settings().clone();
        // The compiler owner replaces outputSelection only. All original
        // code-generation settings must survive that projection unchanged.
        assert!(
            retained
                .as_object_mut()
                .ok_or("missing settings")?
                .remove("outputSelection")
                .is_some()
        );
        assert_eq!(retained, configuration);
        if discarded_details {
            assert!(
                artifact.compilation_settings()["optimizer"]
                    .get("details")
                    .is_some()
            );
            assert!(
                artifact.metadata()["settings"]["optimizer"]
                    .get("details")
                    .is_none()
            );
            assert_eq!(
                artifact.metadata()["settings"]["optimizer"]["enabled"],
                false
            );
        }
        let material = vec![(address(0x22), artifact)];
        assert!(
            preflight(
                &material,
                EvmNetwork::EthereumMainnet,
                address(0x22),
                "store(uint256)",
                &[json!("42")],
                &[],
            )
            .is_err(),
            "exact old artifact must fail feature eligibility"
        );
    }
    Ok(())
}

#[test]
#[ignore = "requires normal attested compiler helper and official historical compiler JS"]
fn actual_historical_unsafe_source_features_reproduce_but_fail_before_execution()
-> Result<(), Box<dyn std::error::Error>> {
    for (declaration, body) in [
        ("", "unchecked { own = x + 1; }"),
        ("", "delete own; own = x;"),
        ("", "assembly { sstore(0, x) }"),
        ("", "for (uint i = 0; i < x; ++i) { own += i; }"),
        ("uint[] public values;", "own = x;"),
        ("uint immutable unused = 1;", "own = x;"),
        (
            "struct Pair { uint left; uint right; } Pair pair;",
            "own = x;",
        ),
        ("function(uint) internal pointer;", "own = x;"),
        (
            "bytes data;",
            "bytes memory b = new bytes(32); delete b[31]; own = x;",
        ),
    ] {
        let source = format!(
            "pragma solidity 0.8.7; contract Counter {{ uint public own; {declaration} function store(uint x) external {{ {body} }} }}"
        );
        let artifact = compile_historical("Counter", &source, &settings())?;
        if declaration.contains("immutable") {
            assert!(
                artifact.immutable_bindings().is_empty(),
                "unused immutable has no runtime references"
            );
        }
        let material = vec![(address(0x22), artifact)];
        assert!(
            preflight(
                &material,
                EvmNetwork::EthereumMainnet,
                address(0x22),
                "store(uint256)",
                &[json!("42")],
                &[]
            )
            .is_err(),
            "{declaration} {body}"
        );
    }
    for source in [
        "pragma solidity 0.8.7; pragma experimental ABIEncoderV2; contract Counter { uint public own; function store(uint x) external { own = x; } }",
        "pragma solidity 0.8.7; contract Parent { uint public own; } contract Counter is Parent { function store(uint x) external { own = x; } }",
    ] {
        let material = vec![(
            address(0x22),
            compile_historical("Counter", source, &settings())?,
        )];
        assert!(
            preflight(
                &material,
                EvmNetwork::EthereumMainnet,
                address(0x22),
                "store(uint256)",
                &[json!("42")],
                &[]
            )
            .is_err()
        );
    }
    Ok(())
}

#[test]
#[ignore = "requires normal attested compiler helper and official historical compiler JS"]
fn actual_historical_external_interface_ignored_reference_result_fails_static_eligibility()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "pragma solidity 0.8.7; interface IDynamic { function store(uint x) external returns (string memory); } contract Counter { function store(uint x) external { IDynamic(0x4444444444444444444444444444444444444444).store(x); } }";
    let scalar_leaf = "pragma solidity 0.8.7; contract Leaf { uint public result; function store(uint x) external { result = x; } }";
    let material = vec![
        (
            address(0x22),
            compile_historical("Counter", source, &settings())?,
        ),
        (
            address(0x44),
            compile_historical("Leaf", scalar_leaf, &settings())?,
        ),
    ];
    assert!(
        preflight(
            &material,
            EvmNetwork::EthereumMainnet,
            address(0x22),
            "store(uint256)",
            &[json!("42")],
            &[]
        )
        .is_err()
    );
    Ok(())
}

#[test]
#[ignore = "requires normal attested compiler helper and both official compiler JS files"]
fn actual_historical_inline_contract_array_cannot_cross_into_modern_aggregate_callee()
-> Result<(), Box<dyn std::error::Error>> {
    let entry_source = "pragma solidity 0.8.7; interface ILeaf {} interface ICall { function accept(ILeaf[1] calldata values, uint x) external; } contract Counter { uint public own; function store(uint x) external { own = x; ICall(0x4444444444444444444444444444444444444444).accept([ILeaf(0x5555555555555555555555555555555555555555)], x); } }";
    let leaf_source = "pragma solidity 0.8.38; contract ModernLeaf { uint public result; function accept(address[1] calldata, uint x) external { result = x; } }";
    let input = CompilationInput::new(
        "0.8.38",
        "ModernLeaf.sol",
        "ModernLeaf",
        json!({"language":"Solidity","sources":{"ModernLeaf.sol":{"content":leaf_source}},"settings":{"optimizer":{"enabled":true,"runs":200},"viaIR":false,"evmVersion":"cancun"}}),
    )?;
    let js = std::fs::read(
        std::env::var_os("MINUTUS_TEST_OFFICIAL_SOLC_JS")
            .ok_or("official modern compiler required")?,
    )?;
    let artifact = TrustedCompiler::packaged()?.reproduce(&input, &js, &AtomicBool::new(false))?;
    let material = vec![
        (
            address(0x22),
            compile_historical("Counter", entry_source, &settings())?,
        ),
        (address(0x44), Arc::new(artifact)),
    ];
    assert!(
        preflight(
            &material,
            EvmNetwork::EthereumMainnet,
            address(0x22),
            "store(uint256)",
            &[json!("42")],
            &[]
        )
        .is_err()
    );
    Ok(())
}
