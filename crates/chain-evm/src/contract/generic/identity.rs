//! Current proved code and executed delegation graph; no creation assumptions.
use super::{error::GenericError, proxy};
use crate::{EvmNetwork, local_simulation::ProvedState};
use alloy_primitives::{Address, U256, keccak256};
use evm_simulation::{CallObservation, ReplayCall, ReplayCallKind};
use serde::Serialize;
use sha2::{Digest, Sha256};
use solidity_compilation::ReproducedArtifact;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

/// One exact current code identity. Labels and compiler ABI confer no safety claim.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenericCodeIdentity {
    /// Actual address from which code executes.
    pub address: String,
    /// Ethereum Keccak of all runtime bytes, including metadata.
    pub runtime_hash: String,
    /// Reproduced compiler commitment, exact ERC-1167 or proved empty-code identity.
    pub source_commitment: String,
    /// Exact compiler build, absent for standard minimal proxies or empty code.
    pub compiler_version: Option<String>,
}

/// Current source/code correspondence and bounded observed delegation identity.
/// This is not a claim about constructor invariants or future implementation code.
pub struct GenericContractIdentity {
    network: EvmNetwork,
    entrypoint: String,
    contracts: Vec<GenericCodeIdentity>,
    artifacts: BTreeMap<Address, Arc<ReproducedArtifact>>,
    layouts: BTreeMap<Address, Address>,
    controls: BTreeMap<(Address, U256), U256>,
    minimal: BTreeMap<Address, Address>,
    potential_implementations: BTreeMap<Address, Address>,
    commitment: String,
}
impl GenericContractIdentity {
    /// Explicit source-only preflight, with no manufactured execution trace.
    /// ERC-1967 layout delegation needs an actual trusted invocation; a decoy
    /// standard slot cannot select an implementation's field interpretation.
    pub(crate) fn from_source_state(
        network: EvmNetwork,
        entrypoint: Address,
        state: &ProvedState,
        artifacts: &[(Address, Arc<ReproducedArtifact>)],
    ) -> Result<Self, GenericError> {
        if artifacts.len() > 16 {
            return Err(GenericError::Limit);
        }
        let supplied = artifacts
            .iter()
            .map(|(a, b)| (*a, Arc::clone(b)))
            .collect::<BTreeMap<_, _>>();
        if supplied.len() != artifacts.len() {
            return Err(GenericError::SourceMismatch);
        }
        let entry = state
            .account(entrypoint)
            .filter(|v| v.exists() && !v.code().is_empty())
            .ok_or(GenericError::MissingState)?;
        let mut addresses = BTreeSet::from([entrypoint]);
        let mut minimal = BTreeMap::new();
        let mut potential_implementations = BTreeMap::new();
        let mut controls = BTreeMap::new();
        if let Some(implementation) = proxy::minimal_target(entry.code()) {
            addresses.insert(implementation);
            minimal.insert(entrypoint, implementation);
        } else if let Some(value) = state.slot(entrypoint, proxy::IMPLEMENTATION)
            && value != U256::ZERO
        {
            let implementation = proxy::slot_address(value)?;
            if implementation == entrypoint {
                return Err(GenericError::UnsupportedProxy);
            }
            for slot in [proxy::IMPLEMENTATION, proxy::ADMIN, proxy::BEACON] {
                controls.insert(
                    (entrypoint, slot),
                    state
                        .slot(entrypoint, slot)
                        .ok_or(GenericError::MissingState)?,
                );
            }
            if controls[&(entrypoint, proxy::BEACON)] != U256::ZERO {
                return Err(GenericError::UnsupportedProxy);
            }
            addresses.insert(implementation);
            potential_implementations.insert(entrypoint, implementation);
        }
        let mut contracts = Vec::new();
        let mut active = BTreeMap::new();
        let mut layouts = BTreeMap::new();
        for address in addresses {
            if proxy::system_address(address) {
                return Err(GenericError::OpaqueEffects);
            }
            let account = state
                .account(address)
                .filter(|v| v.exists() && !v.code().is_empty())
                .ok_or(GenericError::MissingState)?;
            let (source_commitment, compiler_version) = if minimal.contains_key(&address) {
                ("erc1167-exact-standard-runtime-v1".into(), None)
            } else {
                let artifact = supplied.get(&address).ok_or(GenericError::SourceMismatch)?;
                if artifact.runtime() != account.code() {
                    return Err(GenericError::SourceMismatch);
                }
                active.insert(address, Arc::clone(artifact));
                layouts.insert(address, address);
                (
                    artifact.commitment().into(),
                    Some(artifact.compiler_version().into()),
                )
            };
            contracts.push(GenericCodeIdentity {
                address: format!("{address:#x}"),
                runtime_hash: format!("{:#x}", keccak256(account.code())),
                source_commitment,
                compiler_version,
            });
        }
        if let Some(implementation) = minimal.get(&entrypoint) {
            layouts.insert(entrypoint, *implementation);
        }
        let material = serde_json::json!({"domain":"minutus-generic-source-only-v1","network":network.descriptor().chain_id(),"entry":format!("{entrypoint:#x}"),"contracts":contracts,"potential":potential_implementations.iter().map(|(a,b)|(format!("{a:#x}"),format!("{b:#x}"))).collect::<Vec<_>>(),"controls":controls.iter().map(|((a,s),v)|(format!("{a:#x}"),format!("{s:#066x}"),format!("{v:#066x}"))).collect::<Vec<_>>()});
        let commitment = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&material).map_err(|_| GenericError::SourceMismatch)?
            )
        );
        let identity = Self {
            network,
            entrypoint: format!("{entrypoint:#x}"),
            contracts,
            artifacts: active,
            layouts,
            controls,
            minimal,
            potential_implementations,
            commitment,
        };
        identity.verify_state(state)?;
        Ok(identity)
    }
    pub(super) fn from_state(
        network: EvmNetwork,
        entrypoint: Address,
        state: &ProvedState,
        artifacts: &[(Address, Arc<ReproducedArtifact>)],
        calls: &[CallObservation],
        frames: &[ReplayCall],
        writes: &[(Address, U256)],
    ) -> Result<Self, GenericError> {
        if calls.is_empty()
            || calls.len() > 128
            || calls.len() != frames.len()
            || artifacts.len() > 16
            || writes.len() > 128
        {
            return Err(GenericError::Limit);
        }
        let mut compiled = BTreeMap::new();
        for (address, artifact) in artifacts {
            if compiled.insert(*address, Arc::clone(artifact)).is_some() {
                return Err(GenericError::SourceMismatch);
            }
        }
        let addresses = call_addresses(network, entrypoint, calls, frames)?;
        let CodeGraph {
            contracts,
            minimal,
            active,
        } = match_code_graph(state, &compiled, &addresses, entrypoint)?;
        let mut controls = BTreeMap::new();
        let mut layouts = BTreeMap::new();
        for call in calls {
            let storage = parse(&call.target)?;
            let code = parse(&call.code_address)?;
            if storage == code {
                layouts.entry(storage).or_insert(code);
                continue;
            }
            if let Some(expected) = minimal.get(&storage) {
                if *expected != code {
                    return Err(GenericError::UnsupportedProxy);
                }
            } else {
                // Source correspondence plus the actual trace, not the mere
                // presence of a standard slot, establishes this invocation.
                if !active.contains_key(&storage) {
                    return Err(GenericError::UnsupportedProxy);
                }
                for slot in [proxy::IMPLEMENTATION, proxy::ADMIN, proxy::BEACON] {
                    let value = state
                        .slot(storage, slot)
                        .ok_or(GenericError::MissingState)?;
                    controls.insert((storage, slot), value);
                }
                if controls[&(storage, proxy::BEACON)] != U256::ZERO
                    || proxy::slot_address(controls[&(storage, proxy::IMPLEMENTATION)])? != code
                {
                    return Err(GenericError::UnsupportedProxy);
                }
            }
            if let Some(previous) = layouts.insert(storage, code)
                && previous != storage
                && previous != code
            {
                return Err(GenericError::UnsupportedProxy);
            }
        }
        for (_, slot) in writes {
            if proxy::control(*slot) {
                return Err(GenericError::UnsupportedProxy);
            }
        }
        let material = serde_json::json!({"domain":"minutus-generic-source-v1","network":network.descriptor().chain_id(),"entry":format!("{entrypoint:#x}"),"contracts":contracts,"layouts":layouts.iter().map(|(a,b)|(format!("{a:#x}"),format!("{b:#x}"))).collect::<Vec<_>>(),"controls":controls.iter().map(|((a,s),v)|(format!("{a:#x}"),format!("{s:#066x}"),format!("{v:#066x}"))).collect::<Vec<_>>()});
        let commitment = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&material).map_err(|_| GenericError::SourceMismatch)?
            )
        );
        let identity = Self {
            network,
            entrypoint: format!("{entrypoint:#x}"),
            contracts,
            artifacts: active,
            layouts,
            controls,
            minimal,
            potential_implementations: BTreeMap::new(),
            commitment,
        };
        identity.verify_state(state)?;
        Ok(identity)
    }
    /// Exact application entry address, retaining proxy and implementation separately.
    #[must_use]
    pub fn entrypoint(&self) -> &str {
        &self.entrypoint
    }
    /// Exact supported network bound to this proved source identity.
    #[must_use]
    pub const fn network(&self) -> EvmNetwork {
        self.network
    }
    /// Complete qualified code graph. Source-only preflight can include a
    /// potential implementation and makes no execution/dispatch assertion.
    #[must_use]
    pub fn contracts(&self) -> &[GenericCodeIdentity] {
        &self.contracts
    }
    /// Exact code/source/delegation/control-state commitment.
    #[must_use]
    pub fn commitment(&self) -> &str {
        &self.commitment
    }
    /// Complete public ABI/layout metadata with no authority or source comments.
    ///
    /// # Errors
    /// Rejects malformed identities or projections over 64 KiB; never truncates.
    pub fn source_metadata(&self) -> Result<super::GenericSourceMetadata, GenericError> {
        use super::source_metadata::{
            GenericSourceContractMetadata, GenericSourceMetadata, GenericSourceRole,
        };
        let entry = parse(&self.entrypoint)?;
        let mut contracts = Vec::new();
        for code in &self.contracts {
            let address = parse(&code.address)?;
            let role = if address == entry {
                if self.minimal.contains_key(&entry) {
                    GenericSourceRole::MinimalProxy
                } else if self.potential_implementations.contains_key(&entry) {
                    GenericSourceRole::PotentialProxy
                } else if self.controls.contains_key(&(entry, proxy::IMPLEMENTATION)) {
                    GenericSourceRole::ExecutedProxy
                } else {
                    GenericSourceRole::Entrypoint
                }
            } else if self
                .potential_implementations
                .values()
                .any(|target| *target == address)
            {
                GenericSourceRole::PotentialImplementation
            } else if self
                .layouts
                .iter()
                .any(|(owner, target)| *owner != address && *target == address)
            {
                GenericSourceRole::Implementation
            } else if code.source_commitment == "evm-proved-empty-code-v1" {
                GenericSourceRole::EmptyCode
            } else {
                GenericSourceRole::Dependency
            };
            let artifact = self.artifact(address);
            contracts.push(GenericSourceContractMetadata {
                address: code.address.clone(),
                role,
                runtime_hash: code.runtime_hash.clone(),
                source_commitment: code.source_commitment.clone(),
                compiler_version: code.compiler_version.clone(),
                abi: artifact.map(|a| a.abi().clone()),
                storage_layout: artifact.map(|a| a.storage_layout().clone()),
                method_identifiers: artifact.map(|a| a.methods().clone()),
            });
        }
        let projection = GenericSourceMetadata {
            entrypoint: self.entrypoint.clone(),
            contracts,
        };
        if serde_json::to_vec(&projection)
            .map_err(|_| GenericError::Limit)?
            .len()
            > 64 * 1024
        {
            return Err(GenericError::Limit);
        }
        Ok(projection)
    }
    /// Reproduced source artifact for one actual code address.
    pub(crate) fn artifact(&self, address: Address) -> Option<&ReproducedArtifact> {
        self.artifacts.get(&address).map(Arc::as_ref)
    }
    /// Artifact interpreting the code which wrote an exact storage owner.
    pub(crate) fn storage_artifact(&self, address: Address) -> Option<&ReproducedArtifact> {
        self.layouts
            .get(&address)
            .and_then(|code| self.artifact(*code))
    }
    pub(crate) fn entry_artifact(&self) -> Result<&ReproducedArtifact, GenericError> {
        let entry = parse(&self.entrypoint)?;
        self.storage_artifact(entry)
            .ok_or(GenericError::SourceMismatch)
    }
    /// Unsigned ABI discovery only: a proved potential ERC-1967 address is not
    /// a dispatch or implementation-layout certificate. Actual replay must
    /// independently execute the selected implementation before admission.
    pub(crate) fn intent_artifact(
        &self,
        signature: &str,
    ) -> Result<&ReproducedArtifact, GenericError> {
        let entry = parse(&self.entrypoint)?;
        let mut addresses = BTreeSet::new();
        if let Some(address) = self.layouts.get(&entry) {
            addresses.insert(*address);
        }
        if let Some(address) = self.potential_implementations.get(&entry) {
            addresses.insert(*address);
        }
        let choices = addresses
            .into_iter()
            .filter_map(|address| self.artifact(address))
            .filter(|artifact| artifact.methods().get(signature).is_some())
            .collect::<Vec<_>>();
        if choices.len() != 1 {
            return Err(GenericError::InvalidAbi);
        }
        Ok(choices[0])
    }
    pub(super) fn mutable_dispatch(&self) -> bool {
        let Ok(entry) = parse(&self.entrypoint) else {
            return true;
        };
        self.potential_implementations.contains_key(&entry)
            || self.controls.keys().any(|(address, slot)| {
                format!("{address:#x}") == self.entrypoint && *slot == proxy::IMPLEMENTATION
            })
    }
    /// Host-only revalidation against a newly proved common snapshot.
    pub(crate) fn verify_state(&self, state: &ProvedState) -> Result<(), GenericError> {
        for contract in &self.contracts {
            let address = parse(&contract.address)?;
            let code = state
                .account(address)
                .ok_or(GenericError::MissingState)?
                .code();
            if contract.source_commitment != "evm-proved-empty-code-v1" && code.is_empty() {
                return Err(GenericError::SourceMismatch);
            }
            if format!("{:#x}", keccak256(code)) != contract.runtime_hash {
                return Err(GenericError::SourceMismatch);
            }
            if let Some(artifact) = self.artifact(address)
                && artifact.runtime() != code
            {
                return Err(GenericError::SourceMismatch);
            }
            if let Some(target) = self.minimal.get(&address)
                && proxy::minimal_target(code) != Some(*target)
            {
                return Err(GenericError::UnsupportedProxy);
            }
        }
        for ((address, slot), value) in &self.controls {
            if state.slot(*address, *slot) != Some(*value) {
                return Err(GenericError::SnapshotMismatch);
            }
        }
        Ok(())
    }
    pub(crate) fn state_queries(&self) -> Vec<evm_simulation::StateQuery> {
        let mut queries = self
            .contracts
            .iter()
            .map(|c| evm_simulation::StateQuery::Account {
                address: c.address.clone(),
            })
            .collect::<Vec<_>>();
        queries.extend(self.controls.keys().map(|(address, slot)| {
            evm_simulation::StateQuery::Storage {
                address: format!("{address:#x}"),
                slot: format!("{slot:#066x}"),
            }
        }));
        queries
    }
}
struct CodeGraph {
    contracts: Vec<GenericCodeIdentity>,
    minimal: BTreeMap<Address, Address>,
    active: BTreeMap<Address, Arc<ReproducedArtifact>>,
}
fn call_addresses(
    network: EvmNetwork,
    entrypoint: Address,
    calls: &[CallObservation],
    frames: &[ReplayCall],
) -> Result<BTreeSet<Address>, GenericError> {
    let first = calls.first().ok_or(GenericError::ExecutionRejected)?;
    if parse(&first.target)? != entrypoint
        || parse(&first.code_address)? != entrypoint
        || first.depth != 1
    {
        return Err(GenericError::SnapshotMismatch);
    }
    let mut addresses = BTreeSet::from([entrypoint]);
    for (call, frame) in calls.iter().zip(frames) {
        if frame.kind() == ReplayCallKind::CallCode
            || frame.target() != call.target
            || frame.code_address() != call.code_address
            || frame.caller() != call.caller
            || frame.value() != call.value
            || frame.depth() != call.depth
        {
            return Err(GenericError::UnsupportedProxy);
        }
        if (call.target != call.code_address) != (frame.kind() == ReplayCallKind::DelegateCall) {
            return Err(GenericError::UnsupportedProxy);
        }
        if call.depth == 0 || call.depth > 16 || (network.is_base() && call.value != "0x0") {
            return Err(GenericError::OpaqueEffects);
        }
        addresses.insert(parse(&call.target)?);
        addresses.insert(parse(&call.code_address)?);
    }
    if addresses.len() > 16 {
        return Err(GenericError::Limit);
    }
    Ok(addresses)
}
fn match_code_graph(
    state: &ProvedState,
    compiled: &BTreeMap<Address, Arc<ReproducedArtifact>>,
    addresses: &BTreeSet<Address>,
    entrypoint: Address,
) -> Result<CodeGraph, GenericError> {
    let mut graph = CodeGraph {
        contracts: Vec::new(),
        minimal: BTreeMap::new(),
        active: BTreeMap::new(),
    };
    for address in addresses {
        if proxy::system_address(*address) {
            return Err(GenericError::OpaqueEffects);
        }
        let code = state
            .account(*address)
            .ok_or(GenericError::MissingState)?
            .code();
        if *address == entrypoint && code.is_empty() {
            return Err(GenericError::SourceMismatch);
        }
        let (source_commitment, compiler_version) = if code.is_empty() {
            ("evm-proved-empty-code-v1".into(), None)
        } else if let Some(target) = proxy::minimal_target(code) {
            graph.minimal.insert(*address, target);
            ("erc1167-exact-standard-runtime-v1".into(), None)
        } else {
            let artifact = compiled.get(address).ok_or(GenericError::SourceMismatch)?;
            if artifact.runtime() != code {
                return Err(GenericError::SourceMismatch);
            }
            graph.active.insert(*address, Arc::clone(artifact));
            (
                artifact.commitment().into(),
                Some(artifact.compiler_version().into()),
            )
        };
        graph.contracts.push(GenericCodeIdentity {
            address: format!("{address:#x}"),
            runtime_hash: format!("{:#x}", keccak256(code)),
            source_commitment,
            compiler_version,
        });
    }
    Ok(graph)
}
fn parse(value: &str) -> Result<Address, GenericError> {
    value.parse().map_err(|_| GenericError::SnapshotMismatch)
}
