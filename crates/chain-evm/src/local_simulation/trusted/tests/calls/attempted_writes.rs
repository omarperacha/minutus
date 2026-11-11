//! Host trace corruption must not hide an unchanged or restored extra SSTORE.
use super::*;

#[test]
#[ignore = "requires normal attested compiler helper and embedded unit-test pin section"]
fn actual_compiled_envelope_rejects_unlisted_attempted_writes_without_final_changes()
-> Result<(), Box<dyn std::error::Error>> {
    let artifacts = compile_graph()?;
    let business = scope();
    let fixture = FixtureScope {
        material: &artifacts,
        supplied: &artifacts,
        business: &business,
        proxies: &[],
    };
    for (owner, slot) in [
        (address(0x22), U256::ONE),
        (address(0x44), U256::ONE),
        (address(0x55), U256::ZERO),
        (address(0x44), U256::from_str(CONTROL_SLOTS[0])?),
    ] {
        let (intent, mut replay) = scoped_replay(
            fixture,
            EvmNetwork::EthereumMainnet,
            "routeOnly(uint256)",
            &[json!("42")],
            "0",
        )?;
        assert!(!replay.written_slots().contains(&(owner, slot)));
        // The opaque engine certificate stays genuine and unchanged. This
        // crate-private corruption models an erroneous extra host trace record
        // with no final storage delta; candidate admission must still reject it.
        replay.written.push((owner, slot));
        assert!(
            GenericContractCandidate::from_replay_in_scope(intent, replay, &artifacts, &business)
                .is_err()
        );
    }
    Ok(())
}
