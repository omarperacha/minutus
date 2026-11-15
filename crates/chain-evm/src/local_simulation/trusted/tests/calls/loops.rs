//! Modern bounded-loop source proof joins the real maintained execution path.
use super::*;

const SOURCE: &str = r"// SPDX-License-Identifier: MIT
pragma solidity 0.8.38;
contract Batch {
    uint256 public own;
    address private mutableTarget;
    bool private enabled;
    uint256 private padding;
    mapping(uint256 => uint256) public entries;
    function range(uint256 count, uint256 value) external {
        for (uint256 i; i < count; ++i) { entries[i] = value + i; }
    }
    function batch(uint256[] calldata keys, uint256[] calldata values) external {
        require(keys.length == values.length);
        for (uint256 i; i < keys.length; ++i) { entries[keys[i]] = values[i]; }
    }
    function stateBound(uint256 value) external {
        for (uint256 i; i < own; ++i) { entries[i] = value; }
    }
    function probe(uint256 value) external pure returns (uint256) { return value + 1; }
    function current() external view returns (uint256) { return own; }
}
";
fn compile() -> Result<Artifacts, Box<dyn std::error::Error>> {
    let js = std::fs::read(
        std::env::var_os("MINUTUS_TEST_OFFICIAL_SOLC_JS").ok_or("official compiler required")?,
    )?;
    let input = CompilationInput::new(
        "0.8.38",
        "Batch.sol",
        "Batch",
        json!({"language":"Solidity","sources":{"Batch.sol":{"content":SOURCE}},"settings":{"optimizer":{"enabled":true,"runs":200},"viaIR":false,"evmVersion":"cancun"}}),
    )?;
    let artifact = TrustedCompiler::packaged()?.reproduce(&input, &js, &AtomicBool::new(false))?;
    assert_eq!(
        artifact.storage_layout()["storage"]
            .as_array()
            .and_then(|fields| fields.iter().find(|f| f["label"] == "entries"))
            .map(|f| &f["slot"]),
        Some(&json!("3"))
    );
    Ok(vec![(address(0x22), Arc::new(artifact))])
}
fn prove_slots(witness: &Witness) {
    for key in 0..=32u64 {
        let mut input = U256::from(key).to_be_bytes::<32>().to_vec();
        input.extend(U256::from(3).to_be_bytes::<32>());
        assert_eq!(
            witness
                .state
                .slot(address(0x22), U256::from_be_bytes(keccak256(input).0)),
            Some(U256::ZERO)
        );
    }
}
fn values(candidate: &GenericContractCandidate) -> BTreeMap<Vec<String>, Value> {
    candidate
        .effects()
        .storage_changes()
        .iter()
        .flat_map(|word| {
            assert_eq!(word.storage_address, format!("{:#x}", address(0x22)));
            word.fields
                .iter()
                .map(|field| (field.path.clone(), field.after.clone()))
        })
        .collect()
}
fn preflight(
    artifacts: &Artifacts,
    function: &str,
    arguments: &[Value],
) -> Result<(), Box<dyn std::error::Error>> {
    let witness = witness::build_with_proxies(artifacts, EvmNetwork::EthereumMainnet, &[])?;
    prove_slots(&witness);
    let identity = GenericContractIdentity::from_source_state(
        EvmNetwork::EthereumMainnet,
        address(0x22),
        &witness.state,
        artifacts,
    )?;
    let intent = generic_contract_intent(
        &identity,
        &format!("{:#x}", address(0x11)),
        "0",
        function,
        arguments,
    )?;
    admit_generic_transition(&identity, &intent)?;
    Ok(())
}

#[test]
#[ignore = "requires normal attested compiler helper and official compiler JS"]
fn actual_compiled_bounded_loops_match_proved_mapping_effects_on_every_network()
-> Result<(), Box<dyn std::error::Error>> {
    let artifacts = compile()?;
    let business = vec![format!("{:#x}", address(0x22))];
    for network in [
        EvmNetwork::EthereumMainnet,
        EvmNetwork::EthereumSepolia,
        EvmNetwork::BaseMainnet,
        EvmNetwork::BaseSepolia,
    ] {
        prove_slots(&witness::build_with_proxies(&artifacts, network, &[])?);
        for (function, arguments) in [
            ("range(uint256,uint256)", vec![json!("4"), json!("42")]),
            (
                "batch(uint256[],uint256[])",
                vec![json!(["0", "1", "2", "3"]), json!(["42", "43", "44", "45"])],
            ),
        ] {
            let candidate =
                scoped_candidate(&artifacts, network, function, &arguments, "0", &business)?;
            let expected = (0..4u64)
                .map(|key| {
                    (
                        vec!["entries".into(), key.to_string()],
                        json!((42 + key).to_string()),
                    )
                })
                .collect::<BTreeMap<_, _>>();
            assert_eq!(values(&candidate), expected);
            assert_eq!(candidate.replay().written_slots().len(), 4);
            assert_eq!(candidate.replay().calls().len(), 1);
            assert_eq!(candidate.envelope().relations().len(), 4);
        }
    }
    for (function, arguments) in [
        ("range(uint256,uint256)", vec![json!("33"), json!("42")]),
        (
            "batch(uint256[],uint256[])",
            vec![json!(["1", "1"]), json!(["42", "43"])],
        ),
        ("stateBound(uint256)", vec![json!("42")]),
    ] {
        assert!(
            preflight(&artifacts, function, &arguments).is_err(),
            "{function}"
        );
    }
    Ok(())
}

#[test]
#[ignore = "requires normal attested compiler helper and official compiler JS"]
fn actual_compiled_root_view_and_pure_discovery_stay_data_only()
-> Result<(), Box<dyn std::error::Error>> {
    let artifacts = compile()?;
    let network = EvmNetwork::EthereumMainnet;
    let witness = witness::build_with_proxies(&artifacts, network, &[])?;
    let identity = GenericContractIdentity::from_source_state(
        network,
        address(0x22),
        &witness.state,
        &artifacts,
    )?;
    let account = format!("{:#x}", address(0x11));
    for (function, arguments) in [("probe(uint256)", vec![json!("42")]), ("current()", vec![])] {
        assert!(generic_contract_intent(&identity, &account, "0", function, &arguments).is_err());
        let call = crate::contract::DeclaredContractCall::new(
            identity.entrypoint(),
            function,
            &arguments,
            &["uint256".into()],
        )?;
        let intent =
            crate::contract::UnsignedContractIntent::new_declared(network, &account, "0", call)?;
        let description =
            crate::contract::generic::inspect_generic_invocation(&identity, &intent, &account)?;
        assert_eq!(description.function, function);
        assert_eq!(description.arguments, arguments);
        assert!(description.required_calls.is_empty());
    }
    Ok(())
}
