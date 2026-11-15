//! Receipt accounting is historical metadata, not admission to execute an old VM.
use super::{REVIEW_EXPIRES, SEPOLIA_AMSTERDAM};
use crate::{SimulationError, SimulationNetwork};

/// Reviewed relationship between receipt transaction gas and header block gas.
/// This public data projection grants no execution or proof capability.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReceiptGasAccounting {
    /// Header gas accumulates the same charged transaction gas as receipts.
    TransactionTotal,
    /// EIP-8037/7778: receipts sum charged total transaction gas, while header
    /// gas is max(sum of pre-refund regular gas, sum of state gas).
    AmsterdamDimensions,
}

/// Selects receipt accounting for a known network and canonical header time.
/// Historical pre-VM-support blocks retain their classic receipt relationship;
/// this does not make them admissible to modern local execution. Activation and
/// review expiry are shared with the execution owner, never provider flags.
///
/// # Errors
/// Future timestamps beyond the maintained rules review reject closed.
pub fn receipt_gas_accounting(
    network: SimulationNetwork,
    timestamp: u64,
) -> Result<ReceiptGasAccounting, SimulationError> {
    if timestamp >= REVIEW_EXPIRES {
        return Err(SimulationError::UnsupportedRules);
    }
    Ok(match network {
        SimulationNetwork::EthereumSepolia if timestamp >= SEPOLIA_AMSTERDAM => {
            ReceiptGasAccounting::AmsterdamDimensions
        }
        // Pinned Base v1.5.3 keeps Amsterdam disabled. The reviewed Ethereum
        // mainnet interval also retains transaction-total block accounting.
        SimulationNetwork::EthereumMainnet
        | SimulationNetwork::EthereumSepolia
        | SimulationNetwork::BaseMainnet
        | SimulationNetwork::BaseSepolia => ReceiptGasAccounting::TransactionTotal,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receipt_history_and_activation_do_not_expand_execution_support() {
        for network in [
            SimulationNetwork::EthereumMainnet,
            SimulationNetwork::EthereumSepolia,
            SimulationNetwork::BaseMainnet,
            SimulationNetwork::BaseSepolia,
        ] {
            assert_eq!(
                receipt_gas_accounting(network, 0),
                Ok(ReceiptGasAccounting::TransactionTotal)
            );
            assert!(super::super::rules(network, 0).is_err());
            assert_eq!(
                receipt_gas_accounting(network, REVIEW_EXPIRES),
                Err(SimulationError::UnsupportedRules)
            );
            assert_eq!(
                receipt_gas_accounting(network, u64::MAX),
                Err(SimulationError::UnsupportedRules)
            );
        }
        assert_eq!(
            receipt_gas_accounting(SimulationNetwork::EthereumSepolia, SEPOLIA_AMSTERDAM - 1),
            Ok(ReceiptGasAccounting::TransactionTotal)
        );
        assert_eq!(
            receipt_gas_accounting(SimulationNetwork::EthereumSepolia, SEPOLIA_AMSTERDAM),
            Ok(ReceiptGasAccounting::AmsterdamDimensions)
        );
        for network in [
            SimulationNetwork::EthereumMainnet,
            SimulationNetwork::BaseMainnet,
            SimulationNetwork::BaseSepolia,
        ] {
            assert_eq!(
                receipt_gas_accounting(network, SEPOLIA_AMSTERDAM),
                Ok(ReceiptGasAccounting::TransactionTotal)
            );
        }
    }
}
