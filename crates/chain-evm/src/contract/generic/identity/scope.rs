//! Descriptive source graph composition. Fresh proof and execution remain
//! separate requirements; merging identities never creates signing authority.
use super::{EvmNetwork, GenericContractIdentity, GenericError, ProvedState, parse, proxy};
use alloy_primitives::Address;
use sha2::{Digest, Sha256};
use solidity_compilation::ReproducedArtifact;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

impl GenericContractIdentity {
    pub(crate) fn compiled_artifacts(&self) -> Vec<(Address, Arc<ReproducedArtifact>)> {
        self.artifacts
            .iter()
            .map(|(address, artifact)| (*address, Arc::clone(artifact)))
            .collect()
    }
    pub(crate) fn merge_sources(&self, others: &[&Self]) -> Result<Self, GenericError> {
        if others.len() > 16 {
            return Err(GenericError::Limit);
        }
        let mut merged = Self {
            network: self.network,
            entrypoint: self.entrypoint.clone(),
            contracts: Vec::new(),
            artifacts: BTreeMap::new(),
            layouts: BTreeMap::new(),
            controls: BTreeMap::new(),
            minimal: BTreeMap::new(),
            potential_implementations: BTreeMap::new(),
            commitment: String::new(),
        };
        let mut contracts = BTreeMap::new();
        for identity in std::iter::once(self).chain(others.iter().copied()) {
            if identity.network != self.network {
                return Err(GenericError::SnapshotMismatch);
            }
            for contract in &identity.contracts {
                if let Some(old) = contracts.insert(contract.address.clone(), contract.clone())
                    && (old.runtime_hash != contract.runtime_hash
                        || old.source_commitment != contract.source_commitment)
                {
                    return Err(GenericError::SourceMismatch);
                }
            }
            for (address, artifact) in &identity.artifacts {
                if let Some(old) = merged.artifacts.insert(*address, Arc::clone(artifact))
                    && old.commitment() != artifact.commitment()
                {
                    return Err(GenericError::SourceMismatch);
                }
            }
            consistent(&mut merged.layouts, &identity.layouts)?;
            consistent(&mut merged.controls, &identity.controls)?;
            consistent(&mut merged.minimal, &identity.minimal)?;
            consistent(
                &mut merged.potential_implementations,
                &identity.potential_implementations,
            )?;
        }
        if contracts.len() > 16 {
            return Err(GenericError::Limit);
        }
        merged.contracts = contracts.into_values().collect();
        merged.commitment = format!("{:x}", Sha256::digest(serde_json::to_vec(
            &serde_json::json!({"domain":"minutus-source-graph-v2","network":self.network.descriptor().chain_id(),"entry":self.entrypoint,"contracts":merged.contracts,"layouts":merged.layouts.iter().map(|(a,b)|(format!("{a:#x}"),format!("{b:#x}"))).collect::<Vec<_>>(),"controls":merged.controls.iter().map(|((a,s),v)|(format!("{a:#x}"),format!("{s:#066x}"),format!("{v:#066x}"))).collect::<Vec<_>>()})
        ).map_err(|_| GenericError::SourceMismatch)?));
        Ok(merged)
    }

    pub(crate) fn from_scoped_source_state(
        network: EvmNetwork,
        entry: Address,
        state: &ProvedState,
        artifacts: &[(Address, Arc<ReproducedArtifact>)],
        business: &[String],
    ) -> Result<Self, GenericError> {
        let addresses = business
            .iter()
            .map(|v| parse(v))
            .collect::<Result<BTreeSet<_>, _>>()?;
        if addresses.is_empty()
            || addresses.len() != business.len()
            || addresses.len() > 16
            || !addresses.contains(&entry)
        {
            return Err(GenericError::InvalidAbi);
        }
        let root = Self::from_source_state(network, entry, state, artifacts)?;
        let others = addresses
            .into_iter()
            .filter(|a| *a != entry)
            .map(|a| Self::from_source_state(network, a, state, artifacts))
            .collect::<Result<Vec<_>, _>>()?;
        root.merge_sources(&others.iter().collect::<Vec<_>>())
    }

    pub(in crate::contract::generic) fn immutable_dispatch(
        &self,
        address: Address,
    ) -> Result<(Address, &ReproducedArtifact), GenericError> {
        if self.potential_implementations.contains_key(&address)
            || self
                .controls
                .contains_key(&(address, proxy::IMPLEMENTATION))
        {
            return Err(GenericError::UnsupportedProxy);
        }
        let code = *self
            .layouts
            .get(&address)
            .ok_or(GenericError::SourceMismatch)?;
        let artifact = self.artifact(code).ok_or(GenericError::SourceMismatch)?;
        Ok((code, artifact))
    }

    pub(crate) fn verify_execution_graph(&self, actual: &Self) -> Result<(), GenericError> {
        for contract in &actual.contracts {
            if !self.contracts.iter().any(|expected| {
                expected.address == contract.address
                    && expected.runtime_hash == contract.runtime_hash
                    && expected.source_commitment == contract.source_commitment
            }) {
                return Err(GenericError::SourceMismatch);
            }
        }
        for (owner, code) in &actual.layouts {
            if self.layouts.get(owner) != Some(code) {
                return Err(GenericError::UnsupportedProxy);
            }
        }
        Ok(())
    }
}

fn consistent<K: Ord + Clone, V: Eq + Clone>(
    into: &mut BTreeMap<K, V>,
    from: &BTreeMap<K, V>,
) -> Result<(), GenericError> {
    for (key, value) in from {
        if let Some(old) = into.insert(key.clone(), value.clone())
            && old != *value
        {
            return Err(GenericError::SnapshotMismatch);
        }
    }
    Ok(())
}
