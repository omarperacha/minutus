//! Authority-free classification for the reviewed application execution scope.
//! Maintained native lookup constructs wrappers only; it never executes them.
use crate::SimulationNetwork;
use alloy_primitives::{Address, address};
use base_evm::{BasePrecompiles, BaseSpecId, BaseUpgrade};

/// Whether a target is excluded from ordinary application source proofs by the
/// current reviewed standard/rollup protocol execution rules. The union is
/// conservative across reviewed forks; it grants no execution or state trust.
#[must_use]
pub fn is_reserved_application_target(network: SimulationNetwork, target: [u8; 20]) -> bool {
    if standard_reserved(target) {
        return true;
    }
    match network {
        SimulationNetwork::EthereumMainnet | SimulationNetwork::EthereumSepolia => false,
        SimulationNetwork::BaseMainnet | SimulationNetwork::BaseSepolia => {
            base_fee_system(target)
                || [BaseUpgrade::Cobalt, BaseUpgrade::Denim]
                    .into_iter()
                    .any(|upgrade| {
                        BasePrecompiles::new_with_spec(BaseSpecId::new(upgrade))
                            .install()
                            .get(&Address::new(target))
                            .is_some()
                    })
        }
    }
}
pub(crate) fn standard_reserved(target: [u8; 20]) -> bool {
    if target[..18] != [0; 18] {
        return false;
    }
    let low = u16::from_be_bytes([target[18], target[19]]);
    (1..=0x20).contains(&low) || low == 0x100
}
pub(crate) fn base_fee_system(target: [u8; 20]) -> bool {
    [
        address!("420000000000000000000000000000000000000f"),
        address!("4200000000000000000000000000000000000015"),
        address!("4200000000000000000000000000000000000019"),
        address!("420000000000000000000000000000000000001a"),
        address!("420000000000000000000000000000000000001b"),
    ]
    .contains(&Address::new(target))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn standard_reserved_union_rejects_precompiles_and_keeps_nearby_apps() {
        for network in [
            SimulationNetwork::EthereumMainnet,
            SimulationNetwork::EthereumSepolia,
            SimulationNetwork::BaseMainnet,
            SimulationNetwork::BaseSepolia,
        ] {
            for value in [1u16, 0x20, 0x100] {
                let mut target = [0; 20];
                target[18..].copy_from_slice(&value.to_be_bytes());
                assert!(is_reserved_application_target(network, target));
            }
            for value in [0x21u16, 0x101] {
                let mut target = [0; 20];
                target[18..].copy_from_slice(&value.to_be_bytes());
                assert!(!is_reserved_application_target(network, target));
            }
            assert!(!is_reserved_application_target(network, [0x44; 20]));
        }
    }
    #[test]
    fn maintained_base_static_dynamic_and_fee_targets_are_network_specific() {
        for target in [
            address!("b200000000000000000000000000000000000001"),
            address!("b200000000000000000001000000000000000001"),
            address!("b20f000000000000000000000000000000000000"),
            address!("8453000000000000000000000000000000000001"),
            address!("8453000000000000000000000000000000000002"),
            address!("813000000000000000000000000000000000aa01"),
            address!("813000000000000000000000000000000000aa02"),
            address!("4200000000000000000000000000000000000015"),
        ] {
            for network in [
                SimulationNetwork::BaseMainnet,
                SimulationNetwork::BaseSepolia,
            ] {
                assert!(
                    is_reserved_application_target(network, target.into_array()),
                    "{target}"
                );
            }
            for network in [
                SimulationNetwork::EthereumMainnet,
                SimulationNetwork::EthereumSepolia,
            ] {
                assert!(
                    !is_reserved_application_target(network, target.into_array()),
                    "{target}"
                );
            }
        }
        for target in [
            address!("b200000000000000000002000000000000000001"),
            address!("b20f000000000000000000000000000000000001"),
            address!("845300000000000000000000000000000000ffff"),
        ] {
            assert!(
                !is_reserved_application_target(
                    SimulationNetwork::BaseMainnet,
                    target.into_array()
                ),
                "{target}"
            );
        }
    }
}
