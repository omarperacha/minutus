//! Generic immutable intent validation, separate from source/effect qualification.
use app_storage::{StorageError, StoredWorkflowDeclaration, StoredWorkflowRead};
use chain_evm::{
    PublicEvmAddress,
    contract::{DeclaredContractCall, UnsignedContractIntent},
};

/// Canonical generic declaration. No source, effect, approval or signing authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContractWorkflowDeclaration {
    stored: StoredWorkflowDeclaration,
    commitment: String,
}

impl ContractWorkflowDeclaration {
    /// Every declared source-field condition for all-step source/type admission.
    /// This validates requests only and never observes or qualifies a value.
    ///
    /// # Errors
    /// Rejects unsupported observation engines or invalid contract references.
    pub fn condition_requests(
        &self,
    ) -> Result<Vec<chain_evm::contract::generic::GenericCondition>, StorageError> {
        let declared = self
            .stored
            .steps
            .iter()
            .flat_map(|step| step.prerequisites.iter().chain(&step.completion))
            .chain(&self.stored.goals)
            .cloned()
            .collect::<Vec<_>>();
        super::declared_qualification::conditions(self, &declared)
    }
    /// Freezes bounded exact ABI calls and declared temporal requirements.
    ///
    /// # Errors
    /// Rejects malformed ABI values, references, addresses and resource excess.
    pub fn new(mut stored: StoredWorkflowDeclaration) -> Result<Self, StorageError> {
        stored.account = canonical_address(&stored.account)?;
        for contract in &mut stored.contracts {
            contract.address = canonical_address(&contract.address)?;
        }
        stored.validate_structure()?;
        let network = super::scope::network(&stored.network)?;
        for step in &mut stored.steps {
            let target = stored
                .contracts
                .iter()
                .find(|contract| contract.id == step.contract)
                .ok_or(StorageError::InvalidRecord("unknown workflow contract"))?;
            let call =
                DeclaredContractCall::new(&target.address, &step.function, &step.arguments, &[])
                    .map_err(|_| {
                        StorageError::InvalidRecord("invalid generic workflow ABI call")
                    })?;
            let intent = UnsignedContractIntent::new_declared(
                network,
                &stored.account,
                &step.value_wei,
                call.clone(),
            )
            .map_err(|_| StorageError::InvalidRecord("invalid generic workflow native value"))?;
            step.arguments = call.arguments().to_vec();
            step.value_wei = intent.value_wei().into();
        }
        for condition in stored
            .steps
            .iter_mut()
            .flat_map(|step| step.prerequisites.iter_mut().chain(&mut step.completion))
            .chain(&mut stored.goals)
        {
            if let StoredWorkflowRead::AbiRead {
                function,
                arguments,
                ..
            } = &mut condition.read
            {
                let target = stored
                    .contracts
                    .iter()
                    .find(|contract| contract.id == condition.contract)
                    .ok_or(StorageError::InvalidRecord(
                        "unknown workflow condition contract",
                    ))?;
                let call = DeclaredContractCall::new(&target.address, function, arguments, &[])
                    .map_err(|_| StorageError::InvalidRecord("invalid workflow condition ABI"))?;
                *arguments = call.arguments().to_vec();
            }
        }
        let commitment = stored.commitment()?;
        Ok(Self { stored, commitment })
    }
    /// Exact structural declaration, never a qualified source/effect capability.
    #[must_use]
    pub const fn intent(&self) -> &StoredWorkflowDeclaration {
        &self.stored
    }
    /// Complete immutable declaration commitment.
    #[must_use]
    pub fn commitment(&self) -> &str {
        &self.commitment
    }
}

fn canonical_address(address: &str) -> Result<String, StorageError> {
    PublicEvmAddress::new(address)
        .map(|address| address.as_str().into())
        .map_err(|_| StorageError::InvalidRecord("invalid workflow address"))
}
