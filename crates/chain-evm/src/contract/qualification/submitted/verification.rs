use super::VerifiedSubmittedTransaction;
use crate::{
    EvmNetwork, EvmRpcUrl,
    contract::qualification::{DeploymentProof, QualificationError, acquisition, inclusion},
    local_simulation,
};
use alloy_consensus::{Header, SignableTransaction};
use alloy_primitives::{Address, B256, keccak256};
use evm_simulation::SimulationControl;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{str::FromStr, sync::atomic::AtomicBool};

struct Expected {
    network: EvmNetwork,
    hash: B256,
    account: Address,
    nonce: u64,
    signing_digest: B256,
}

/// Reconciles an already-submitted transaction using bounded public full-block
/// data from the configured RPC. Both MPT roots, canonical full header, exact
/// type-2 body digest, recovered signer, nonce, chain and receipt are checked.
/// Success and revert are accepted; neither proves a business goal. Expected
/// facts must come from the host's persisted submission, not model assertions.
/// No signing, send, rebroadcast, receipt polling or retry occurs here.
///
/// # Errors
/// Rejects pending/unknown transactions, wrong bindings, unsupported envelopes,
/// malformed proofs, oversized blocks, cancellation, deadlines and reorgs.
pub fn verify_submitted_transaction(
    network: EvmNetwork,
    endpoint: &EvmRpcUrl,
    hash: &str,
    expected_sender: &str,
    expected_nonce: &str,
    expected_signing_digest: &str,
    cancelled: &AtomicBool,
) -> Result<VerifiedSubmittedTransaction, QualificationError> {
    let expected = Expected::parse(
        network,
        hash,
        expected_sender,
        expected_nonce,
        expected_signing_digest,
    )?;
    let control = SimulationControl::start(cancelled);
    let (header, proof) = acquisition::acquire_with_header(network, endpoint, hash, &control)?;
    let verified = expected.verify(&header, &proof)?;
    // Recheck after pure verification too; the same original aggregate deadline
    // covers acquisition, verification and this final canonical observation.
    let repeated =
        local_simulation::canonical_header(endpoint, network, &proof.block_hash, &control)?;
    if repeated.hash_slow() != header.hash_slow() {
        return Err(QualificationError::SnapshotMismatch);
    }
    control.remaining()?;
    Ok(verified)
}

impl Expected {
    fn parse(
        network: EvmNetwork,
        hash: &str,
        sender: &str,
        nonce: &str,
        digest: &str,
    ) -> Result<Self, QualificationError> {
        let reject = QualificationError::InvalidIntent;
        let parsed_nonce = nonce.parse::<u64>().map_err(|_| reject)?;
        let hash_value = B256::from_str(hash).map_err(|_| reject)?;
        let digest_value = B256::from_str(digest).map_err(|_| reject)?;
        if parsed_nonce.to_string() != nonce
            || format!("{hash_value:#x}") != hash
            || format!("{digest_value:#x}") != digest
        {
            return Err(reject);
        }
        Ok(Self {
            network,
            hash: hash_value,
            account: Address::from_str(sender).map_err(|_| reject)?,
            nonce: parsed_nonce,
            signing_digest: digest_value,
        })
    }

    fn verify(
        &self,
        header: &Header,
        proof: &DeploymentProof,
    ) -> Result<VerifiedSubmittedTransaction, QualificationError> {
        let reject = QualificationError::SnapshotMismatch;
        let checked = inclusion::verify(header, &format!("{:#x}", self.hash), proof)?;
        let transaction = checked.signed.tx();
        if transaction.chain_id != self.network.descriptor().chain_id()
            || transaction.nonce != self.nonce
            || keccak256(transaction.encoded_for_signing()) != self.signing_digest
            || checked.signed.recover_signer().map_err(|_| reject)? != self.account
        {
            return Err(reject);
        }
        let commitment = json!({
            "domain": "minutus-exact-submitted-inclusion-v1",
            "network": self.network,
            "hash": format!("{:#x}", self.hash),
            "account": format!("{:#x}", self.account),
            "nonce": self.nonce.to_string(),
            "signingDigest": format!("{:#x}", self.signing_digest),
            "proof": proof,
        });
        Ok(VerifiedSubmittedTransaction {
            network: self.network,
            transaction_hash: format!("{:#x}", self.hash),
            account: format!("{:#x}", self.account),
            nonce: self.nonce.to_string(),
            signing_digest: format!("{:#x}", self.signing_digest),
            succeeded: checked.succeeded,
            block_number: header.number,
            block_hash: format!("{:#x}", header.hash_slow()),
            inclusion_commitment: format!(
                "0x{:x}",
                Sha256::digest(serde_json::to_vec(&commitment).map_err(|_| reject)?)
            ),
        })
    }
}

#[cfg(test)]
#[path = "verification_tests.rs"]
mod tests;
