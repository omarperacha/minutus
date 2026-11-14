use app_storage::{StorageError, StoredWorkflowScope};
use chain_evm::{
    EvmNetwork, PublicEvmAddress,
    contract::{DeclaredContractCall, UnsignedContractIntent},
};

/// Immutable ABI-validated semantic scope. This validates intent shape only;
/// source/profile/effect qualification is a separate opaque chain capability.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContractWorkflowScope {
    stored: StoredWorkflowScope,
    commitment: String,
}
impl ContractWorkflowScope {
    /// Canonicalises full ABI values and freezes the exact ordered semantic scope.
    /// Fresh block, nonce, gas, fees and approval never enter this scope digest.
    ///
    /// # Errors
    /// Rejects invalid ABI/address/value/profile references and resource bounds.
    pub fn new(mut stored: StoredWorkflowScope) -> Result<Self, StorageError> {
        let network = network(&stored.network)?;
        stored.account = PublicEvmAddress::new(&stored.account)
            .map_err(|_| StorageError::InvalidRecord("invalid workflow account"))?
            .as_str()
            .into();
        for intent in &mut stored.steps {
            intent.target = PublicEvmAddress::new(&intent.target)
                .map_err(|_| StorageError::InvalidRecord("invalid workflow target"))?
                .as_str()
                .into();
        }
        stored.validate_structure()?;
        for intent in &mut stored.steps {
            let call = DeclaredContractCall::new(
                &intent.target,
                &intent.operation,
                &intent.arguments,
                &[],
            )
            .map_err(|_| StorageError::InvalidRecord("invalid canonical workflow ABI"))?;
            // Reuse the exact unsigned value range/account validation; it grants
            // no source qualification and cannot become a prepared transaction.
            let unsigned = UnsignedContractIntent::new_declared(
                network,
                &stored.account,
                &intent.value_wei,
                call.clone(),
            )
            .map_err(|_| StorageError::InvalidRecord("invalid workflow native value"))?;
            intent.target = call.target().into();
            intent.arguments = call.arguments().to_vec();
            intent.value_wei = unsigned.value_wei().into();
        }
        let commitment = stored.commitment()?;
        Ok(Self { stored, commitment })
    }
    /// Exact structural scope for native display or persistence, never authority.
    #[must_use]
    pub const fn intent(&self) -> &StoredWorkflowScope {
        &self.stored
    }
    /// Domain-separated immutable scope commitment.
    #[must_use]
    pub fn commitment(&self) -> &str {
        &self.commitment
    }
}
pub(super) fn network(value: &str) -> Result<EvmNetwork, StorageError> {
    EvmNetwork::ALL
        .into_iter()
        .find(|network| network.descriptor().caip2_id() == value)
        .ok_or(StorageError::InvalidRecord("unsupported workflow network"))
}
