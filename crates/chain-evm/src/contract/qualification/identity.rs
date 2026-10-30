use super::{QualificationError, action::parse_address, deployment::VerifiedDeployment, profile};
use crate::EvmNetwork;
use crate::local_simulation::{ProvedSimulation, ProvedState};
use alloy_primitives::Address;
use evm_simulation::{SimulationNetwork, SimulationRequest};

/// Exact code/source correspondence minted from a sealed proof-checked run.
/// Unknown proxies, linked/immutable variants and arbitrary source labels reject.
pub struct QualifiedContractIdentity {
    network: EvmNetwork,
    target: String,
    block_hash: String,
    runtime_hash: &'static str,
    deployment: VerifiedDeployment,
}

impl QualifiedContractIdentity {
    pub(super) fn from_run(
        run: &ProvedSimulation,
        deployment: VerifiedDeployment,
    ) -> Result<Self, QualificationError> {
        Self::from_state(run.request(), run.state(), deployment)
    }
    pub(super) fn from_state(
        request: &SimulationRequest,
        state: &ProvedState,
        deployment: VerifiedDeployment,
    ) -> Result<Self, QualificationError> {
        let target = parse_address(&request.transaction.target)?;
        let network = match request.network {
            SimulationNetwork::EthereumMainnet => EvmNetwork::EthereumMainnet,
            SimulationNetwork::EthereumSepolia => EvmNetwork::EthereumSepolia,
            SimulationNetwork::BaseMainnet => EvmNetwork::BaseMainnet,
            SimulationNetwork::BaseSepolia => EvmNetwork::BaseSepolia,
        };
        Self::from_snapshot(
            network,
            target,
            &request.block.hash,
            request.block.number,
            state,
            deployment,
        )
    }
    pub(super) fn from_snapshot(
        network: EvmNetwork,
        target: Address,
        block_hash: &str,
        block_number: u64,
        state: &ProvedState,
        deployment: VerifiedDeployment,
    ) -> Result<Self, QualificationError> {
        let account = state
            .account(target)
            .ok_or(QualificationError::MissingState)?;
        if !account.exists() {
            return Err(QualificationError::MissingState);
        }
        if deployment.network() != network
            || deployment.target() != target
            || deployment.block_number() > block_number
            || account.nonce() != 1
        {
            return Err(QualificationError::InvalidDeployment);
        }
        profile::verify(account.code())?;
        Ok(Self {
            network,
            target: format!("{target:#x}"),
            block_hash: block_hash.into(),
            runtime_hash: profile::runtime_hash(),
            deployment,
        })
    }
    /// Exact chain qualified by canonical state and creation provenance.
    #[must_use]
    pub const fn network(&self) -> EvmNetwork {
        self.network
    }
    /// Exact observed contract address; never an untrusted token label.
    #[must_use]
    pub fn target(&self) -> &str {
        &self.target
    }
    /// Exact canonical snapshot at qualification.
    #[must_use]
    pub fn block_hash(&self) -> &str {
        &self.block_hash
    }
    /// Complete runtime hash, including compiler metadata.
    #[must_use]
    pub const fn runtime_hash(&self) -> &str {
        self.runtime_hash
    }
    /// Reviewed code-profile identity.
    #[must_use]
    pub const fn profile_id(&self) -> &str {
        profile::ID
    }
    /// Exact source/compiler/settings/ABI/layout revision commitment.
    #[must_use]
    pub const fn profile_revision(&self) -> &str {
        profile::REVISION
    }
    /// Exact successful creation transaction identity, independently included.
    #[must_use]
    pub fn deployment_transaction_hash(&self) -> String {
        format!("{:#x}", self.deployment.transaction_hash())
    }
    /// Canonical creation block, rechecked before fresh qualification.
    #[must_use]
    pub fn deployment_block_hash(&self) -> String {
        format!("{:#x}", self.deployment.block_hash())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{B256, Bytes};
    use alloy_trie::TrieAccount;
    use std::collections::BTreeMap;

    fn state(
        target: Address,
        exists: bool,
        nonce: u64,
        correct_code: bool,
    ) -> Result<ProvedState, QualificationError> {
        let code = if correct_code {
            alloy_primitives::hex::decode(
                include_str!("../../../profiles/erc20-oz-5.7.0/runtime.hex")
                    .trim()
                    .trim_start_matches("0x"),
            )
            .map_err(|_| QualificationError::InvalidArtifact)?
        } else {
            vec![0]
        };
        Ok(ProvedState::test_fixture(
            [(
                target,
                (
                    exists,
                    TrieAccount {
                        nonce,
                        ..Default::default()
                    },
                    Bytes::from(code),
                ),
            )]
            .into_iter()
            .collect(),
            BTreeMap::new(),
        ))
    }

    #[test]
    fn source_identity_needs_no_fake_transaction_and_requires_exact_creation_code_snapshot()
    -> Result<(), QualificationError> {
        let (deployment, target) = VerifiedDeployment::test_fixture()?;
        let block_hash = format!("{:#x}", B256::repeat_byte(0x77));
        let identity = QualifiedContractIdentity::from_snapshot(
            EvmNetwork::EthereumSepolia,
            target,
            &block_hash,
            100,
            &state(target, true, 1, true)?,
            deployment,
        )?;
        assert_eq!(identity.target(), format!("{target:#x}"));
        assert_eq!(identity.block_hash(), block_hash);
        assert_eq!(identity.network(), EvmNetwork::EthereumSepolia);
        for mutation in 0..6 {
            let (deployment, target) = VerifiedDeployment::test_fixture()?;
            let (requested, height, exists, nonce, code) = match mutation {
                0 => (target, 41, true, 1, true),
                1 => (Address::repeat_byte(2), 100, true, 1, true),
                2 => (target, 100, false, 1, true),
                3 => (target, 100, true, 2, true),
                4 => (target, 100, true, 1, false),
                _ => (target, 100, true, 1, true),
            };
            assert!(
                QualifiedContractIdentity::from_snapshot(
                    if mutation == 5 {
                        EvmNetwork::EthereumMainnet
                    } else {
                        EvmNetwork::EthereumSepolia
                    },
                    requested,
                    &block_hash,
                    height,
                    &state(requested, exists, nonce, code)?,
                    deployment
                )
                .is_err(),
                "source mutation {mutation}"
            );
        }
        Ok(())
    }
}
