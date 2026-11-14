use crate::wallet_lifecycle::{TransferCancellation, WalletControl};
use app_core::{ContractWorkflowScope, StoredWorkflowIntent, StoredWorkflowScope};
use chain_evm::{
    EvmNetwork, EvmRpcUrl,
    contract::{
        ContractBlock, DeclaredContractCall, DeploymentProof, QualifiedContractCandidate,
        UnsignedContractIntent, acquire_contract_transaction_proof, prepare_qualified_contract,
    },
    local_simulation::contract_fee_facts,
};
use evm_simulation::TrustedSimulationWorkers;
use ipc_types::IpcError;
use serde::Deserialize;
use std::sync::{Arc, atomic::AtomicBool};

const PROFILE: &str = "oz570-solc0838-cancun-erc20-v1";
const REVISION: &str = "76df8832488cb0beac36a11b032a1240a39f05202b4c604d580e0485645feef7";
/// Proposed exact ordered semantics; source/ABI authority is always host qualified.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ContractWorkflowRequest {
    pub(crate) network: ipc_types::EvmNetwork,
    pub(crate) steps: Vec<ContractWorkflowRequestStep>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ContractWorkflowRequestStep {
    pub(crate) target: String,
    pub(crate) function: String,
    pub(crate) arguments: Vec<serde_json::Value>,
    pub(crate) value_wei: String,
    pub(crate) deployment_transaction: String,
}
pub(crate) fn available() -> bool {
    // Release promotion is intentionally closed until the recorded native
    // interaction qualification and final human security review have passed.
    // Debug qualification builds still require both packaged attested workers;
    // no environment or model-controlled override may change this policy.
    if !cfg!(debug_assertions) {
        return false;
    }
    TrustedSimulationWorkers::packaged().is_ok()
}
impl ContractWorkflowRequest {
    pub(crate) fn validate(&self) -> Result<(), IpcError> {
        self.scope("0x1111111111111111111111111111111111111111")?;
        let mut deployments = std::collections::BTreeMap::new();
        for step in &self.steps {
            chain_evm::TransactionHash::new(&step.deployment_transaction).map_err(|_| {
                IpcError::invalid_request("Expected an exact deployment transaction hash")
            })?;
            if step.value_wei != "0"
                || !matches!(
                    step.function.as_str(),
                    "approve(address,uint256)"
                        | "transfer(address,uint256)"
                        | "transferFrom(address,address,uint256)"
                )
            {
                return Err(IpcError::invalid_request(
                    "A workflow step is outside the reviewed zero-value token profile",
                ));
            }
            let addresses = match step.function.as_str() {
                "transferFrom(address,address,uint256)" => 2,
                _ => 1,
            };
            if step.arguments.iter().take(addresses).any(|arg| {
                arg.as_str().is_none_or(|value| {
                    value.eq_ignore_ascii_case("0x0000000000000000000000000000000000000000")
                })
            }) {
                return Err(IpcError::invalid_request(
                    "The reviewed token profile rejects zero-address operands",
                ));
            }
            let key = step.target.to_ascii_lowercase();
            let hash = step.deployment_transaction.to_ascii_lowercase();
            if deployments
                .insert(key, hash.clone())
                .is_some_and(|old| old != hash)
            {
                return Err(IpcError::invalid_request(
                    "One target cannot name conflicting deployments",
                ));
            }
        }
        Ok(())
    }
    pub(super) fn scope(&self, account: &str) -> Result<ContractWorkflowScope, IpcError> {
        let steps = self
            .steps
            .iter()
            .map(|step| StoredWorkflowIntent {
                target: step.target.clone(),
                profile_id: PROFILE.into(),
                profile_revision: 1,
                profile_commitment: REVISION.into(),
                operation: step.function.clone(),
                arguments: step.arguments.clone(),
                value_wei: step.value_wei.clone(),
            })
            .collect();
        ContractWorkflowScope::new(StoredWorkflowScope {
            version: 1,
            network: crate::evm_rpc_settings::chain_network(self.network)
                .descriptor()
                .caip2_id()
                .into(),
            account: account.into(),
            steps,
        })
        .map_err(|error| crate::error_mapping::app_core_error(error.into()))
    }
}
pub(super) struct Context {
    pub(super) workers: Arc<TrustedSimulationWorkers>,
    pub(super) endpoint: Arc<EvmRpcUrl>,
    pub(super) network: EvmNetwork,
    pub(super) deployments: std::collections::BTreeMap<String, Arc<DeploymentProof>>,
    pub(super) cancelled: Arc<AtomicBool>,
    pub(super) wallet_epoch: u64,
    pub(super) account: String,
    pub(super) origin: crate::evm_approval::ApprovalOrigin,
}
impl Context {
    pub(super) fn check(
        &self,
        wallet: &WalletControl,
        cancel: &TransferCancellation,
    ) -> Result<(), IpcError> {
        cancel.check()?;
        wallet.check_transfer_epoch(self.wallet_epoch)?;
        if !wallet
            .selected_address()?
            .eq_ignore_ascii_case(&self.account)
        {
            return Err(IpcError::invalid_request(
                "The selected account changed; the workflow is stopped",
            ));
        }
        Ok(())
    }
    pub(super) async fn new(
        request: &ContractWorkflowRequest,
        account: String,
        wallet_epoch: u64,
        cancelled: Arc<AtomicBool>,
        origin: crate::evm_approval::ApprovalOrigin,
        endpoint: Arc<EvmRpcUrl>,
    ) -> Result<Self, IpcError> {
        let workers = Arc::new(TrustedSimulationWorkers::packaged().map_err(chain_error)?);
        let network = crate::evm_rpc_settings::chain_network(request.network);
        let mut deployments = std::collections::BTreeMap::new();
        for step in &request.steps {
            let target = step.target.to_ascii_lowercase();
            if deployments.contains_key(&target) {
                continue;
            }
            let proof_endpoint = Arc::clone(&endpoint);
            let hash = step.deployment_transaction.clone();
            let cancellation = Arc::clone(&cancelled);
            let checked_target = target.clone();
            let deployment = tokio::task::spawn_blocking(move || {
                let proof = acquire_contract_transaction_proof(
                    network,
                    &proof_endpoint,
                    &hash,
                    &cancellation,
                )?;
                let identity = chain_evm::contract::qualify_contract_workflow_source(
                    network,
                    &proof_endpoint,
                    &checked_target,
                    &proof,
                    &cancellation,
                )?;
                if identity.network() != network
                    || identity.target() != checked_target
                    || identity.profile_id() != PROFILE
                    || identity.profile_revision() != REVISION
                {
                    return Err(chain_evm::contract::QualificationError::UnsupportedProfile);
                }
                Ok::<_, chain_evm::contract::QualificationError>(proof)
            })
            .await
            .map_err(|_| IpcError::internal())?
            .map_err(chain_error)?;
            deployments.insert(target, Arc::new(deployment));
        }
        Ok(Self {
            workers,
            endpoint,
            network,
            deployments,
            cancelled,
            wallet_epoch,
            account,
            origin,
        })
    }
    pub(super) fn deployment_for(&self, target: &str) -> Result<Arc<DeploymentProof>, IpcError> {
        self.deployments
            .get(target)
            .map(Arc::clone)
            .ok_or_else(|| IpcError::invalid_request("Unadmitted workflow source"))
    }
    pub(super) async fn prepare(
        &self,
        intent: &StoredWorkflowIntent,
    ) -> Result<QualifiedContractCandidate, IpcError> {
        let call = DeclaredContractCall::new(
            &intent.target,
            &intent.operation,
            &intent.arguments,
            &["bool".into()],
        )
        .map_err(chain_error)?;
        let intent = UnsignedContractIntent::new_declared(
            self.network,
            &self.account,
            &intent.value_wei,
            call,
        )
        .map_err(chain_error)?;
        let workers = Arc::clone(&self.workers);
        let endpoint = self.endpoint.clone();
        let deployment = self.deployment_for(intent.call().target())?;
        let cancelled = Arc::clone(&self.cancelled);
        tokio::task::spawn_blocking(move || {
            let fees = contract_fee_facts(intent.network(), &endpoint, &cancelled)?;
            prepare_qualified_contract(
                &workers,
                &endpoint,
                &intent,
                &ContractBlock::Latest,
                fees,
                &deployment,
                &cancelled,
            )
        })
        .await
        .map_err(|_| IpcError::internal())?
        .map_err(chain_error)
    }
}
pub(super) fn chain_error(error: impl std::fmt::Display) -> IpcError {
    IpcError::invalid_request(format!("Contract qualification stopped: {error}"))
}
