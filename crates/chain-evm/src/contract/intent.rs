//! Wire-intent groundwork, deliberately outside prepared/signable transactions.
use super::{ContractCall, ContractError, DeclaredContractCall};
use crate::{EvmNetwork, PublicEvmAddress, rpc::Uint256};

/// Immutable requested write facts. This is neither a prepared transaction nor
/// a capability: source/proxy/effect qualification and all native signing gates
/// must be implemented before any generic write can become available.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnsignedContractIntent {
    network: EvmNetwork,
    account: PublicEvmAddress,
    value_wei: String,
    call: DeclaredContractCall,
}
impl UnsignedContractIntent {
    /// Binds exact chain/account/target/value/function/arguments/calldata without
    /// accepting a signing digest, nonce, gas policy or arbitrary RPC endpoint.
    ///
    /// # Errors
    /// Rejects invalid/zero accounts, noncanonical/out-of-range value and ABI.
    pub fn new(
        network: EvmNetwork,
        account: &str,
        value_wei: &str,
        call: ContractCall,
    ) -> Result<Self, ContractError> {
        let outputs = call
            .outputs()
            .iter()
            .map(|ty| {
                serde_json::to_value(ty)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .ok_or(ContractError::UnsupportedAbi)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let arguments = call
            .arguments()
            .iter()
            .map(|value| serde_json::Value::String(value.clone()))
            .collect::<Vec<_>>();
        let declared =
            DeclaredContractCall::new(call.target(), call.signature(), &arguments, &outputs)?;
        // Preserve the existing consuming API and release its redundant wire facts
        // before validating the replacement declared call.
        drop(call);
        Self::new_declared(network, account, value_wei, declared)
    }

    /// Binds a bounded declared dynamic call without granting signing authority.
    ///
    /// # Errors
    /// Rejects invalid accounts/value or changed/non-canonical calldata.
    pub fn new_declared(
        network: EvmNetwork,
        account: &str,
        value_wei: &str,
        call: DeclaredContractCall,
    ) -> Result<Self, ContractError> {
        let account = PublicEvmAddress::new(account).map_err(|_| ContractError::InvalidAddress)?;
        if account.as_str() == "0x0000000000000000000000000000000000000000" {
            return Err(ContractError::InvalidAddress);
        }
        if value_wei.len() > 78 || (value_wei.len() > 1 && value_wei.starts_with('0')) {
            return Err(ContractError::InvalidArguments);
        }
        let value =
            Uint256::from_decimal(value_wei).map_err(|_| ContractError::InvalidArguments)?;
        call.verify_calldata(call.calldata())?;
        Ok(Self {
            network,
            account,
            value_wei: value.to_decimal(),
            call,
        })
    }
    /// The four-network host identity, never a model-provided chain number.
    #[must_use]
    pub const fn network(&self) -> EvmNetwork {
        self.network
    }
    /// Requested public sender; no proof of account control.
    #[must_use]
    pub fn account(&self) -> &str {
        self.account.as_str()
    }
    /// Exact decimal native value in wei.
    #[must_use]
    pub fn value_wei(&self) -> &str {
        &self.value_wei
    }
    /// Exact independently decoded call facts, still unqualified semantics.
    #[must_use]
    pub const fn call(&self) -> &DeclaredContractCall {
        &self.call
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wire_intent_binds_each_identity_without_granting_signing_authority()
    -> Result<(), ContractError> {
        let account = "0x1111111111111111111111111111111111111111";
        let target = "0x2222222222222222222222222222222222222222";
        let call = ContractCall::new(target, "f(uint256)", &["123".into()], &[])?;
        for network in EvmNetwork::ALL {
            let intent = UnsignedContractIntent::new(network, account, "7", call.clone())?;
            assert_eq!(intent.network(), network);
            assert_eq!(intent.account(), account);
            assert_eq!(intent.value_wei(), "7");
            assert_eq!(intent.call().target(), target);
            assert_eq!(intent.call().arguments(), &[serde_json::json!("123")]);
        }
        for value in ["-1", "01", "1e2", ""] {
            assert!(
                UnsignedContractIntent::new(EvmNetwork::BaseSepolia, account, value, call.clone())
                    .is_err()
            );
        }
        Ok(())
    }
}
