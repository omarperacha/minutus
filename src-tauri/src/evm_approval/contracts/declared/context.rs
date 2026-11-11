//! Native-owned source broker, fixed endpoint and immutable qualified identities.
use crate::{
    evm_approval::ApprovalOrigin,
    wallet_lifecycle::{TransferCancellation, WalletControl},
};
use app_core::{
    ContractWorkflowDeclaration, DeclaredContractCompletion, DeclaredContractWorkflowStep,
    PreparedDeclaredContractStep,
};
use chain_evm::{
    EvmNetwork, EvmRpcUrl,
    contract::{
        ContractBlock,
        generic::{
            GenericContractCandidate, GenericContractIdentity, GenericError, GenericSourceProvider,
            RefreshedGenericCandidate, VerifiedGenericPrerequisites,
            admit_generic_transition_in_scope, bind_generic_prerequisites, generic_condition_type,
            generic_contract_intent, prepare_generic_contract_in_scope,
            qualify_generic_write_source, refresh_generic_contract, verify_generic_conditions,
            verify_predicted_generic_conditions,
        },
    },
    local_simulation::contract_fee_facts,
};
use evm_simulation::TrustedSimulationWorkers;
use ipc_types::IpcError;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex, atomic::AtomicBool},
};

pub(super) struct Context {
    pub(super) workers: Arc<TrustedSimulationWorkers>,
    pub(super) endpoint: Arc<EvmRpcUrl>,
    pub(super) network: EvmNetwork,
    pub(super) cancelled: Arc<AtomicBool>,
    pub(super) wallet_epoch: u64,
    pub(super) account: String,
    pub(super) origin: ApprovalOrigin,
    sources: Arc<Mutex<GenericSourceProvider>>,
    identities: BTreeMap<String, Arc<GenericContractIdentity>>,
    prepared: Mutex<Vec<Arc<GenericContractCandidate>>>,
    observations: Mutex<Vec<crate::evm_approval::contracts::reporting::ObservedFacts>>,
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
                "The selected account changed; the declared workflow is stopped",
            ));
        }
        Ok(())
    }
    pub(super) async fn preflight(
        declaration: &ContractWorkflowDeclaration,
        endpoint: Arc<EvmRpcUrl>,
        cancelled: Arc<AtomicBool>,
        directory: PathBuf,
        origin: ApprovalOrigin,
    ) -> Result<Self, IpcError> {
        let workers = Arc::new(TrustedSimulationWorkers::packaged().map_err(error)?);
        let network = EvmNetwork::ALL
            .into_iter()
            .find(|network| network.descriptor().caip2_id() == declaration.intent().network)
            .ok_or_else(|| IpcError::invalid_request("Unsupported workflow network"))?;
        let contracts = declaration.intent().contracts.clone();
        let declared = declaration.intent().clone();
        let report_declaration = declaration.clone();
        let conditions = declaration
            .condition_requests()
            .map_err(|error| crate::error_mapping::app_core_error(error.into()))?;
        let worker_endpoint = Arc::clone(&endpoint);
        let cancellation = Arc::clone(&cancelled);
        let (sources, identities) = tokio::task::spawn_blocking(move || {
            let mut sources = GenericSourceProvider::packaged(&directory)?;
            let mut identities = BTreeMap::new();
            for contract in contracts {
                let identity = qualify_generic_write_source(
                    network,
                    &worker_endpoint,
                    &contract.address,
                    &mut sources,
                    &cancellation,
                )?;
                if identity.network() != network || identity.entrypoint() != contract.address {
                    return Err(GenericError::SnapshotMismatch);
                }
                identities.insert(contract.id, Arc::new(identity));
            }
            let mut condition_types = Vec::with_capacity(conditions.len());
            for condition in conditions {
                let identity = identities
                    .values()
                    .find(|identity| identity.entrypoint() == condition.target)
                    .ok_or(GenericError::SourceMismatch)?;
                let ty = generic_condition_type(identity, &condition)?;
                condition_types.push((condition, ty));
            }
            let views = identities.values().map(Arc::as_ref).collect::<Vec<_>>();
            for step in &declared.steps {
                let identity = identities
                    .get(&step.contract)
                    .ok_or(GenericError::SourceMismatch)?;
                let intent = generic_contract_intent(
                    identity,
                    &declared.account,
                    &step.value_wei,
                    &step.function,
                    &step.arguments,
                )?;
                admit_generic_transition_in_scope(identity, &views, &intent)?;
            }
            super::report_budget::admit(&report_declaration, &condition_types)?;
            Ok::<_, GenericError>((sources, identities))
        })
        .await
        .map_err(|_| IpcError::internal())?
        .map_err(error)?;
        Ok(Self {
            workers,
            endpoint,
            network,
            cancelled,
            wallet_epoch: 0,
            account: declaration.intent().account.clone(),
            origin,
            sources: Arc::new(Mutex::new(sources)),
            identities,
            prepared: Mutex::new(Vec::new()),
            observations: Mutex::new(Vec::new()),
        })
    }
    pub(super) async fn prepare(
        &self,
        work: &DeclaredContractWorkflowStep,
        historical: &[Arc<DeclaredContractCompletion>],
    ) -> Result<(GenericContractCandidate, VerifiedGenericPrerequisites), IpcError> {
        let identity = Arc::clone(
            self.identities
                .get(&work.intent().contract)
                .ok_or_else(|| IpcError::invalid_request("Unadmitted declared contract"))?,
        );
        let step = work.intent().clone();
        let account = self.account.clone();
        let network = self.network;
        let endpoint = Arc::clone(&self.endpoint);
        let workers = Arc::clone(&self.workers);
        let sources = Arc::clone(&self.sources);
        let cancelled = Arc::clone(&self.cancelled);
        let identities = self.identities.values().cloned().collect::<Vec<_>>();
        let prepared = self.candidate_views()?;
        let previous = historical.to_vec();
        let conditions = work
            .prerequisites()
            .map_err(|error| crate::error_mapping::app_core_error(error.into()))?;
        let completion = work
            .completion_conditions()
            .map_err(|error| crate::error_mapping::app_core_error(error.into()))?;
        let business = work
            .declaration()
            .intent()
            .contracts
            .iter()
            .map(|contract| contract.address.clone())
            .collect::<Vec<_>>();
        let result = tokio::task::spawn_blocking(move || {
            let mut views = identities.iter().map(Arc::as_ref).collect::<Vec<_>>();
            for previous in &prepared {
                if !views
                    .iter()
                    .any(|identity| identity.commitment() == previous.identity().commitment())
                {
                    views.push(previous.identity());
                }
            }
            let observed =
                verify_generic_conditions(&endpoint, network, &views, &conditions, &cancelled)?;
            let intent = generic_contract_intent(
                &identity,
                &account,
                &step.value_wei,
                &step.function,
                &step.arguments,
            )?;
            let fees = contract_fee_facts(network, &endpoint, &cancelled)
                .map_err(|_| GenericError::SnapshotMismatch)?;
            let candidate = prepare_generic_contract_in_scope(
                &workers,
                &endpoint,
                &intent,
                &ContractBlock::Hash(observed.block_hash().into()),
                fees,
                &mut *sources
                    .lock()
                    .map_err(|_| GenericError::ExecutionRejected)?,
                &cancelled,
                &business,
            )?;
            if !views
                .iter()
                .any(|identity| identity.commitment() == candidate.identity().commitment())
            {
                views.push(candidate.identity());
            }
            verify_predicted_generic_conditions(
                &endpoint,
                &candidate,
                &views,
                &completion,
                &cancelled,
            )?;
            let observed =
                verify_generic_conditions(&endpoint, network, &views, &conditions, &cancelled)?;
            if observed.block_hash() != candidate.block_hash() {
                return Err(GenericError::SnapshotMismatch);
            }
            let previous = previous
                .iter()
                .map(|completion| completion.proof())
                .collect::<Vec<_>>();
            let gate =
                bind_generic_prerequisites(&endpoint, &candidate, &previous, observed, &cancelled)?;
            Ok::<_, GenericError>((candidate, gate))
        })
        .await
        .map_err(|_| IpcError::internal())?
        .map_err(error)?;
        self.record_conditions(result.1.conditions())?;
        Ok(result)
    }
    pub(super) async fn refresh(
        &self,
        step: &PreparedDeclaredContractStep,
    ) -> Result<RefreshedGenericCandidate, IpcError> {
        let workers = Arc::clone(&self.workers);
        let endpoint = Arc::clone(&self.endpoint);
        let candidate = step.shared_candidate();
        let sources = Arc::clone(&self.sources);
        let cancelled = Arc::clone(&self.cancelled);
        let identities = self.identity_views();
        let prepared = self.candidate_views()?;
        let completion = step
            .completion_conditions()
            .map_err(|error| crate::error_mapping::app_core_error(error.into()))?;
        tokio::task::spawn_blocking(move || {
            let fresh = refresh_generic_contract(
                &workers,
                &endpoint,
                &candidate,
                &mut *sources
                    .lock()
                    .map_err(|_| GenericError::ExecutionRejected)?,
                &cancelled,
            )?;
            let mut views = identities.iter().map(Arc::as_ref).collect::<Vec<_>>();
            for identity in prepared
                .iter()
                .map(|candidate| candidate.identity())
                .chain([candidate.identity(), fresh.fresh().identity()])
            {
                if !views
                    .iter()
                    .any(|known| known.commitment() == identity.commitment())
                {
                    views.push(identity);
                }
            }
            // Ancillary condition fields can change while the transaction's
            // own effects remain material-equivalent. Recheck the trusted fresh
            // poststate before either signing or transport validation.
            verify_predicted_generic_conditions(
                &endpoint,
                fresh.fresh(),
                &views,
                &completion,
                &cancelled,
            )?;
            Ok::<_, GenericError>(fresh)
        })
        .await
        .map_err(|_| IpcError::internal())?
        .map_err(error)
    }
    pub(super) async fn prerequisites(
        &self,
        step: &PreparedDeclaredContractStep,
        historical: &[Arc<DeclaredContractCompletion>],
    ) -> Result<VerifiedGenericPrerequisites, IpcError> {
        let candidate = step.shared_candidate();
        let identities = self.identities.values().cloned().collect::<Vec<_>>();
        let prepared = self.candidate_views()?;
        let historical = historical.to_vec();
        let conditions = step
            .prerequisites()
            .map_err(|error| crate::error_mapping::app_core_error(error.into()))?;
        let endpoint = Arc::clone(&self.endpoint);
        let network = self.network;
        let cancelled = Arc::clone(&self.cancelled);
        let proof = tokio::task::spawn_blocking(move || {
            let mut identities = identities.iter().map(Arc::as_ref).collect::<Vec<_>>();
            for previous in &prepared {
                if !identities
                    .iter()
                    .any(|identity| identity.commitment() == previous.identity().commitment())
                {
                    identities.push(previous.identity());
                }
            }
            if !identities
                .iter()
                .any(|identity| identity.commitment() == candidate.identity().commitment())
            {
                identities.push(candidate.identity());
            }
            let observed = verify_generic_conditions(
                &endpoint,
                network,
                &identities,
                &conditions,
                &cancelled,
            )?;
            let historical = historical
                .iter()
                .map(|completion| completion.proof())
                .collect::<Vec<_>>();
            bind_generic_prerequisites(&endpoint, &candidate, &historical, observed, &cancelled)
        })
        .await
        .map_err(|_| IpcError::internal())?
        .map_err(error)?;
        self.record_conditions(proof.conditions())?;
        Ok(proof)
    }
    pub(super) fn identity_views(&self) -> Vec<Arc<GenericContractIdentity>> {
        self.identities.values().cloned().collect()
    }
    pub(super) fn remember(
        &self,
        candidate: Arc<GenericContractCandidate>,
    ) -> Result<(), IpcError> {
        let mut prepared = self.prepared.lock().map_err(|_| IpcError::internal())?;
        if prepared.len() >= 16
            || prepared
                .iter()
                .any(|old| old.candidate_digest() == candidate.candidate_digest())
        {
            return Err(IpcError::invalid_request(
                "Generic workflow candidate history changed",
            ));
        }
        prepared.push(candidate);
        Ok(())
    }
    pub(super) fn candidate_views(&self) -> Result<Vec<Arc<GenericContractCandidate>>, IpcError> {
        self.prepared
            .lock()
            .map(|prepared| prepared.clone())
            .map_err(|_| IpcError::internal())
    }
    pub(super) fn record_conditions(
        &self,
        proof: &chain_evm::contract::generic::VerifiedGenericConditions,
    ) -> Result<(), IpcError> {
        let mut observations = self.observations.lock().map_err(|_| IpcError::internal())?;
        if observations.len() >= 128 {
            return Err(IpcError::invalid_request(
                "Workflow observation history limit reached",
            ));
        }
        observations.push(super::reporting::capture(proof)?);
        Ok(())
    }
    pub(super) fn observation_snapshots(
        &self,
    ) -> Result<Vec<crate::evm_approval::contracts::reporting::ObservedFacts>, IpcError> {
        self.observations
            .lock()
            .map(|observed| observed.clone())
            .map_err(|_| IpcError::internal())
    }
}
pub(super) fn error(error: impl std::fmt::Display) -> IpcError {
    IpcError::invalid_request(format!("Generic contract qualification stopped: {error}"))
}
