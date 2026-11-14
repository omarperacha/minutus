//! Fixed minimal dispatch composes at both caller and callee boundaries.
use super::*;

#[test]
#[ignore = "requires normal attested compiler helper and embedded unit-test pin section"]
fn actual_compiled_call_graph_matches_fixed_proxy_frames_and_storage_owners()
-> Result<(), Box<dyn std::error::Error>> {
    let original = compile_graph()?;
    for mask in [1u8, 2, 3] {
        let mut artifacts = original.clone();
        let mut proxies = Vec::new();
        for (bit, index, owner, implementation) in [(1, 0, 0x22, 0x66), (2, 1, 0x44, 0x77)] {
            if mask & bit != 0 {
                artifacts[index].0 = address(implementation);
                proxies.push((address(owner), address(implementation)));
            }
        }
        for network in [
            EvmNetwork::EthereumMainnet,
            EvmNetwork::EthereumSepolia,
            EvmNetwork::BaseMainnet,
            EvmNetwork::BaseSepolia,
        ] {
            let candidate = scoped_candidate_with(
                FixtureScope {
                    material: &artifacts,
                    supplied: &artifacts,
                    business: &scope(),
                    proxies: &proxies,
                },
                network,
                "routeContext(uint256)",
                &[json!("42")],
                "0",
            )?;
            assert_eq!(field(&candidate, address(0x44), "result")?, &json!("42"));
            assert_eq!(
                field(&candidate, address(0x44), "sender")?,
                &json!(format!("{:#x}", address(0x22)))
            );
            assert_eq!(
                field(&candidate, address(0x44), "origin")?,
                &json!(format!("{:#x}", address(0x11)))
            );
            let calls = candidate.replay().calls();
            assert_eq!(calls.len(), 2 + proxies.len());
            assert_eq!(
                calls
                    .iter()
                    .filter(|frame| frame.kind() == ReplayCallKind::DelegateCall)
                    .count(),
                proxies.len()
            );
            assert!(
                candidate
                    .effects()
                    .storage_changes()
                    .iter()
                    .all(|word| word.storage_address == format!("{:#x}", address(0x44)))
            );
        }
    }
    Ok(())
}
