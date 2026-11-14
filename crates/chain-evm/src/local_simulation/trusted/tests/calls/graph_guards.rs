//! Static source-closure checks deliberately do not invoke an execution engine.
use super::*;
use crate::contract::generic::admit_generic_transition_in_scope;

fn preflight(
    artifacts: &Artifacts,
    network: EvmNetwork,
    entry: Address,
    function: &str,
    arguments: &[Value],
    proxies: &[(Address, Address)],
) -> Result<(), Box<dyn std::error::Error>> {
    let witness = witness::build_with_proxies(artifacts, network, proxies)?;
    let identity =
        GenericContractIdentity::from_source_state(network, entry, &witness.state, artifacts)?;
    let others = artifacts
        .iter()
        .map(|(owner, _)| *owner)
        .chain(proxies.iter().map(|(owner, _)| *owner))
        .filter(|owner| *owner != entry && !proxies.iter().any(|(_, code)| owner == code))
        .map(|owner| {
            GenericContractIdentity::from_source_state(network, owner, &witness.state, artifacts)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let intent = generic_contract_intent(
        &identity,
        &format!("{:#x}", address(0x11)),
        "0",
        function,
        arguments,
    )?;
    admit_generic_transition_in_scope(&identity, &others.iter().collect::<Vec<_>>(), &intent)?;
    Ok(())
}

#[test]
#[ignore = "requires normal attested compiler helper and embedded unit-test pin section"]
fn actual_compiled_graph_rejects_mutable_routing_aliases_and_ignored_failure_before_execution()
-> Result<(), Box<dyn std::error::Error>> {
    let artifacts = compile_graph()?;
    preflight(
        &artifacts,
        EvmNetwork::EthereumMainnet,
        address(0x22),
        "route(uint256)",
        &[json!("42")],
        &[],
    )?;
    for function in [
        "mutableRoute(uint256)",
        "stateBranch(uint256)",
        "overwrite(uint256)",
        "ignoredFailure(uint256)",
        "pay(uint256)",
    ] {
        assert!(
            preflight(
                &artifacts,
                EvmNetwork::EthereumMainnet,
                address(0x22),
                function,
                &[json!("42")],
                &[]
            )
            .is_err(),
            "{function}"
        );
    }
    Ok(())
}

#[test]
#[ignore = "requires normal attested compiler helper and embedded unit-test pin section"]
fn actual_compiled_source_cannot_override_reserved_native_storage_or_code_targets()
-> Result<(), Box<dyn std::error::Error>> {
    let original = compile_graph()?;
    for (network, target, reserved) in [
        (
            EvmNetwork::EthereumMainnet,
            "0000000000000000000000000000000000000100",
            true,
        ),
        (
            EvmNetwork::EthereumMainnet,
            "0000000000000000000000000000000000000101",
            false,
        ),
        (
            EvmNetwork::BaseMainnet,
            "b200000000000000000000000000000000000001",
            true,
        ),
        (
            EvmNetwork::BaseMainnet,
            "8453000000000000000000000000000000000001",
            true,
        ),
        (
            EvmNetwork::BaseSepolia,
            "845300000000000000000000000000000000ffff",
            false,
        ),
    ] {
        let target = Address::from_str(target)?;
        let mut artifacts = original.clone();
        artifacts[1].0 = target;
        let result = preflight(
            &artifacts,
            network,
            address(0x22),
            "argumentTarget(address,uint256)",
            &[json!(format!("{target:#x}")), json!("42")],
            &[],
        );
        assert_eq!(result.is_err(), reserved, "storage target {target}");
        // The business address itself is ordinary, but its fixed code target
        // must still be classified before source is treated as application code.
        let result = preflight(
            &artifacts,
            network,
            address(0x22),
            "routeOnly(uint256)",
            &[json!("42")],
            &[(address(0x44), target)],
        );
        assert_eq!(result.is_err(), reserved, "code target {target}");
    }
    Ok(())
}
