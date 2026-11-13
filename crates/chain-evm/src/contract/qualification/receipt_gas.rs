//! Receipt sanity bounds use the same reviewed fork schedule as execution.
use super::QualificationError;
use crate::EvmNetwork;
use alloy_consensus::Header;
use evm_simulation::{ReceiptGasAccounting, SimulationNetwork, receipt_gas_accounting};

pub(super) fn validate(
    network: EvmNetwork,
    header: &Header,
    cumulative: u64,
) -> Result<(), QualificationError> {
    let network = match network {
        EvmNetwork::EthereumMainnet => SimulationNetwork::EthereumMainnet,
        EvmNetwork::EthereumSepolia => SimulationNetwork::EthereumSepolia,
        EvmNetwork::BaseMainnet => SimulationNetwork::BaseMainnet,
        EvmNetwork::BaseSepolia => SimulationNetwork::BaseSepolia,
    };
    let accounting = receipt_gas_accounting(network, header.timestamp)?;
    let multiplier = match accounting {
        ReceiptGasAccounting::TransactionTotal => 1,
        ReceiptGasAccounting::AmsterdamDimensions => 2,
    };
    // Amsterdam txGas <= regularGas + stateGas, while headerGas is the max
    // of those block totals. Thus receipt cumulative <= 2*headerGas remains
    // conservative. u128 prevents overflow; both MPT roots remain mandatory.
    let maximum = u128::from(header.gas_used) * multiplier;
    if cumulative == 0 || u128::from(cumulative) > maximum {
        return Err(QualificationError::SnapshotMismatch);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn historical_classic_and_reviewed_amsterdam_bounds_remain_distinct() {
        let historical = Header {
            gas_used: 10,
            ..Header::default()
        };
        for network in EvmNetwork::ALL {
            assert!(validate(network, &historical, 10).is_ok());
            assert!(validate(network, &historical, 11).is_err());
            assert!(validate(network, &historical, 0).is_err());
        }
        // Timestamp is the captured public header's time, not a duplicated
        // activation constant. The leaf owner tests the exact fork boundary.
        let modern = Header {
            timestamp: 1_794_576_300,
            gas_used: 10,
            ..Header::default()
        };
        assert!(validate(EvmNetwork::EthereumSepolia, &modern, 20).is_ok());
        assert!(validate(EvmNetwork::EthereumSepolia, &modern, 21).is_err());
        for network in [
            EvmNetwork::EthereumMainnet,
            EvmNetwork::BaseMainnet,
            EvmNetwork::BaseSepolia,
        ] {
            assert!(validate(network, &modern, 11).is_err());
        }
        let maximum = Header {
            gas_used: u64::MAX,
            ..modern.clone()
        };
        assert!(validate(EvmNetwork::EthereumSepolia, &maximum, u64::MAX).is_ok());
        let unknown = Header {
            timestamp: u64::MAX,
            ..modern
        };
        assert!(validate(EvmNetwork::EthereumSepolia, &unknown, 1).is_err());
    }
}
