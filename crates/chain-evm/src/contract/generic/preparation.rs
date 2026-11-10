//! Host-owned source acquisition and generic candidate preparation. Declarations
//! never choose ABI outputs, fees, nonce, providers or compiler executables.
use super::{
    GenericContractCandidate, GenericContractIdentity, GenericError, GenericSourceProvider, abi,
    proxy,
};
use crate::{
    EvmNetwork, EvmRpcUrl,
    contract::{ContractBlock, DeclaredContractCall, UnsignedContractIntent},
    local_simulation::{self, SimulationFeeFacts},
};
use alloy_primitives::{Address, U256};
use evm_simulation::{SimulationControl, StateQuery, TrustedSimulationWorkers};
use serde_json::Value;
use std::{str::FromStr, sync::atomic::AtomicBool};

/// Locally reproduces every source needed for a bounded current target identity.
/// It creates no effect, approval, freshness or signing authority. Exact minimal
/// proxies and proved potential ERC-1967 implementations retain separate roles.
/// Native outbound source/RPC consent must precede this operation.
///
/// # Errors
/// Rejects missing proof/source/compiler data, unknown code, invalid proxy slots,
/// source mismatch, unsupported compilation and resource/cancellation failures.
pub fn qualify_generic_source(
    network: EvmNetwork,
    endpoint: &EvmRpcUrl,
    target: &str,
    sources: &mut GenericSourceProvider,
    cancelled: &AtomicBool,
) -> Result<GenericContractIdentity, GenericError> {
    qualify_generic_source_at(
        network,
        endpoint,
        target,
        &ContractBlock::Latest,
        sources,
        cancelled,
    )
}

/// Source correspondence at an explicitly selected canonical snapshot. This
/// produces descriptive identity only; it makes no current-tip or signing claim.
///
/// # Errors
/// Rejects unavailable proof/source/compiler material or noncanonical snapshots.
pub fn qualify_generic_source_at(
    network: EvmNetwork,
    endpoint: &EvmRpcUrl,
    target: &str,
    block: &ContractBlock,
    sources: &mut GenericSourceProvider,
    cancelled: &AtomicBool,
) -> Result<GenericContractIdentity, GenericError> {
    qualify_source_at(network, endpoint, target, block, sources, cancelled)
        .map(|(identity, _, _)| identity)
}

/// Read-only source metadata tied to its own exact proved canonical snapshot.
/// It contains no source identity handle, effect certificate or signing authority.
pub struct GenericSourceObservation {
    network: EvmNetwork,
    target: String,
    block_hash: String,
    block_number: u64,
    metadata: super::GenericSourceMetadata,
}
impl GenericSourceObservation {
    /// Exact closed network observed by the source owner.
    #[must_use]
    pub const fn network(&self) -> EvmNetwork {
        self.network
    }
    /// Exact source entry address, retaining proxy roles in metadata.
    #[must_use]
    pub fn target(&self) -> &str {
        &self.target
    }
    /// Canonical observation hash, independently of any other inspect call.
    #[must_use]
    pub fn block_hash(&self) -> &str {
        &self.block_hash
    }
    /// Canonical observation height.
    #[must_use]
    pub const fn block_number(&self) -> u64 {
        self.block_number
    }
    /// Complete bounded compiler-derived ABI/layout and code-role projection.
    #[must_use]
    pub const fn metadata(&self) -> &super::GenericSourceMetadata {
        &self.metadata
    }
}

/// Inspects source correspondence at the exact requested block without wallet,
/// signature or write authority. Latest always resolves afresh; explicit hashes
/// remain explicit and canonical. No truncation or source-status boolean grants
/// authority to a future write.
///
/// # Errors
/// Rejects unknown source/compiler/code, block inconsistency and resource limits.
pub fn inspect_generic_source(
    network: EvmNetwork,
    endpoint: &EvmRpcUrl,
    target: &str,
    block: &ContractBlock,
    sources: &mut GenericSourceProvider,
    cancelled: &AtomicBool,
) -> Result<GenericSourceObservation, GenericError> {
    let (identity, block_number, block_hash) =
        qualify_source_at(network, endpoint, target, block, sources, cancelled)?;
    Ok(GenericSourceObservation {
        network,
        target: identity.entrypoint().into(),
        block_hash,
        block_number,
        metadata: identity.source_metadata()?,
    })
}

fn qualify_source_at(
    network: EvmNetwork,
    endpoint: &EvmRpcUrl,
    target: &str,
    block: &ContractBlock,
    sources: &mut GenericSourceProvider,
    cancelled: &AtomicBool,
) -> Result<(GenericContractIdentity, u64, String), GenericError> {
    let address = Address::from_str(target).map_err(|_| GenericError::SourceMismatch)?;
    if address == Address::ZERO {
        return Err(GenericError::SourceMismatch);
    }
    let control = SimulationControl::start(cancelled);
    let header = match block {
        ContractBlock::Latest => local_simulation::latest_header(endpoint, network, &control),
        ContractBlock::Hash(hash) => {
            local_simulation::canonical_header(endpoint, network, hash, &control)
        }
    }
    .map_err(state_error)?;
    let hash = format!("{:#x}", header.hash_slow());
    let mut queries = account_controls(address);
    let (_, initial) = local_simulation::read_proved(endpoint, network, &hash, &queries, &control)
        .map_err(state_error)?;
    let code = initial
        .account(address)
        .ok_or(GenericError::MissingState)?
        .code();
    let implementation = if let Some(minimal) = proxy::minimal_target(code) {
        Some(minimal)
    } else {
        match initial.slot(address, proxy::IMPLEMENTATION) {
            Some(value) if value != U256::ZERO => Some(proxy::slot_address(value)?),
            _ => None,
        }
    };
    let state = if let Some(implementation) = implementation {
        if implementation == address {
            return Err(GenericError::UnsupportedProxy);
        }
        queries.extend(account_controls(implementation));
        local_simulation::read_proved(endpoint, network, &hash, &queries, &control)
            .map_err(state_error)?
            .1
    } else {
        initial
    };
    let mut artifacts = Vec::new();
    for address in state.account_addresses() {
        let code = state
            .account(address)
            .ok_or(GenericError::MissingState)?
            .code();
        if code.is_empty() || proxy::minimal_target(code).is_some() {
            continue;
        }
        artifacts.push((
            address,
            sources.artifact(network, &format!("{address:#x}"), code, cancelled)?,
        ));
    }
    // Source compilation can be slow and confers no freshness. A separate fresh
    // execution snapshot is required when this identity prepares a candidate.
    let control = SimulationControl::start(cancelled);
    local_simulation::canonical_header(endpoint, network, &hash, &control).map_err(state_error)?;
    let identity =
        GenericContractIdentity::from_source_state(network, address, &state, &artifacts)?;
    Ok((identity, header.number, hash))
}

/// Derives outputs and payable constraints exclusively from locally reproduced
/// ABI. The resulting unsigned intent is still not source/effect admission.
///
/// # Errors
/// Rejects ambiguity, unknown functions, malformed values or source ABI mismatch.
pub fn generic_contract_intent(
    identity: &GenericContractIdentity,
    account: &str,
    value_wei: &str,
    signature: &str,
    arguments: &[Value],
) -> Result<UnsignedContractIntent, GenericError> {
    let artifact = identity.intent_artifact(signature)?;
    let selected = artifact
        .abi()
        .as_array()
        .ok_or(GenericError::InvalidAbi)?
        .iter()
        .filter(|item| item["type"] == "function")
        .find(|item| abi::signature(item).is_ok_and(|name| name == signature))
        .ok_or(GenericError::InvalidAbi)?;
    let outputs = abi::parameters(&selected["outputs"])?;
    let call = DeclaredContractCall::new(identity.entrypoint(), signature, arguments, &outputs)
        .map_err(|_| GenericError::InvalidAbi)?;
    let intent = UnsignedContractIntent::new_declared(identity.network(), account, value_wei, call)
        .map_err(|_| GenericError::InvalidAbi)?;
    abi::bind(artifact.abi(), &intent)?;
    Ok(intent)
}

/// Prepares a complete typed candidate through trusted proof-backed replay at
/// the host-selected block, independently of any worker-reported effects.
///
/// # Errors
/// Rejects unknown/incomplete source/effects, reverts, missing state, invalid
/// contexts, cancellation or any fixed execution/review bound.
pub fn prepare_generic_contract(
    workers: &TrustedSimulationWorkers,
    endpoint: &EvmRpcUrl,
    intent: &UnsignedContractIntent,
    block: &ContractBlock,
    fees: SimulationFeeFacts,
    sources: &mut GenericSourceProvider,
    cancelled: &AtomicBool,
) -> Result<GenericContractCandidate, GenericError> {
    let (identity, _, hash) = qualify_source_at(
        intent.network(),
        endpoint,
        intent.call().target(),
        block,
        sources,
        cancelled,
    )?;
    let envelope = super::admit_generic_transition(&identity, intent)?;
    let preload = relation_queries(endpoint, &identity, intent, &hash, &envelope, cancelled)?;
    let control = SimulationControl::start(cancelled);
    let replay = local_simulation::replay_proved(
        workers,
        endpoint,
        intent,
        &ContractBlock::Hash(hash),
        fees,
        &preload,
        &control,
    )
    .map_err(state_error)?;
    let mut artifacts = Vec::new();
    let mut reached = std::collections::BTreeSet::new();
    for call in &replay.observation().calls {
        for value in [&call.target, &call.code_address] {
            reached.insert(Address::from_str(value).map_err(|_| GenericError::SnapshotMismatch)?);
        }
    }
    // Proved potential implementations also participate in selector-interception
    // rejection even when the proxy did not actually delegate this invocation.
    for address in replay.state().account_addresses() {
        if !proxy::system_address(address)
            && replay
                .state()
                .account(address)
                .is_some_and(|account| !account.code().is_empty())
        {
            reached.insert(address);
        }
    }
    if reached.len() > 16 {
        return Err(GenericError::Limit);
    }
    for address in reached {
        let code = replay
            .state()
            .account(address)
            .ok_or(GenericError::MissingState)?
            .code();
        if code.is_empty() || proxy::minimal_target(code).is_some() {
            continue;
        }
        artifacts.push((
            address,
            sources.artifact(intent.network(), &format!("{address:#x}"), code, cancelled)?,
        ));
    }
    let candidate = GenericContractCandidate::from_replay(intent.clone(), replay, &artifacts)?;
    // Reproduction may outlast a reorg. Recheck the exact header after all
    // reached-source work, while retaining the replay's original freshness age.
    let control = SimulationControl::start(cancelled);
    local_simulation::canonical_header(
        endpoint,
        intent.network(),
        candidate.block_hash(),
        &control,
    )
    .map_err(state_error)?;
    if !candidate.is_fresh() {
        return Err(GenericError::Expired);
    }
    Ok(candidate)
}
fn relation_queries(
    endpoint: &EvmRpcUrl,
    identity: &GenericContractIdentity,
    intent: &UnsignedContractIntent,
    hash: &str,
    envelope: &super::GenericEffectEnvelope,
    cancelled: &AtomicBool,
) -> Result<Vec<StateQuery>, GenericError> {
    let control = SimulationControl::start(cancelled);
    let address =
        Address::from_str(intent.call().target()).map_err(|_| GenericError::InvalidAbi)?;
    let artifact = identity
        .storage_artifact(address)
        .ok_or(GenericError::SourceMismatch)?;
    let mut reader =
        local_simulation::ProvedReader::new(endpoint, intent.network(), hash, &control)
            .map_err(state_error)?;
    let mut queries = Vec::new();
    for relation in envelope.relations() {
        super::layout::read_condition_path(
            artifact.storage_layout(),
            &relation.path,
            address,
            |query| {
                let StateQuery::Storage { .. } = &query else {
                    return Err(GenericError::OpaqueEffects);
                };
                if !queries.contains(&query) && queries.len() >= 128 {
                    return Err(GenericError::Limit);
                }
                let evm_simulation::StateValue::Storage { value } =
                    reader.query(&query, &control).map_err(state_error)?
                else {
                    return Err(GenericError::MissingState);
                };
                let value = U256::from_str(&value).map_err(|_| GenericError::MissingState)?;
                if !queries.contains(&query) {
                    queries.push(query);
                }
                Ok(value)
            },
        )?;
    }
    reader.finish(&control).map_err(state_error)?;
    Ok(queries)
}
fn account_controls(address: Address) -> Vec<StateQuery> {
    let mut queries = vec![StateQuery::Account {
        address: format!("{address:#x}"),
    }];
    queries.extend(
        [proxy::IMPLEMENTATION, proxy::ADMIN, proxy::BEACON]
            .into_iter()
            .map(|slot| StateQuery::Storage {
                address: format!("{address:#x}"),
                slot: format!("{slot:#066x}"),
            }),
    );
    queries
}
fn state_error(error: evm_simulation::SimulationError) -> GenericError {
    match error {
        evm_simulation::SimulationError::Cancelled => GenericError::Expired,
        evm_simulation::SimulationError::Limit => GenericError::Limit,
        evm_simulation::SimulationError::StateUnavailable => GenericError::MissingState,
        _ => GenericError::ExecutionRejected,
    }
}
