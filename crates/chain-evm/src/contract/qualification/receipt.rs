//! Receipt inclusion and independently proved current goal assertions. A receipt
//! alone never demonstrates a task outcome or attributes whole-block deltas.
use super::{
    DeploymentProof, QualificationError, QualifiedContractCandidate,
    action::{allowance_slot, balance_slot, parse_address},
    deployment, profile,
};
use crate::{EvmNetwork, EvmRpcUrl, local_simulation};
use alloy_consensus::{
    Eip658Value, Header, Receipt, ReceiptWithBloom, SignableTransaction, Signed, TxEip1559,
};
use alloy_primitives::{Address, B256, U256, keccak256};
use alloy_rlp::Decodable;
use alloy_trie::{Nibbles, proof::verify_proof};
use evm_simulation::{SimulationControl, StateQuery};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    str::FromStr,
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

/// Goal assertion under the compiled direct-token profile, never a raw slot claim.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ContractPredicate {
    /// Exact remaining authority, including explicit maximum allowance.
    Allowance {
        /// Exact owner.
        owner: String,
        /// Exact spender.
        spender: String,
        /// Exact raw allowance.
        amount: String,
    },
    /// Independently observed balance assertion, not transaction attribution.
    TokenBalance {
        /// Exact account.
        account: String,
        /// Exact bounded comparison.
        comparison: PredicateComparison,
        /// Exact raw units.
        amount: String,
    },
}
/// Bounded current-value comparison.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PredicateComparison {
    /// Exact equality.
    Equal,
    /// Minimum balance.
    AtLeast,
    /// Maximum balance.
    AtMost,
}
impl ContractPredicate {
    fn word(&self) -> Result<(U256, U256), QualificationError> {
        let (slot, amount) = match self {
            Self::Allowance {
                owner,
                spender,
                amount,
            } => (
                allowance_slot(parse_address(owner)?, parse_address(spender)?),
                amount,
            ),
            Self::TokenBalance {
                account, amount, ..
            } => (balance_slot(parse_address(account)?), amount),
        };
        let expected =
            U256::from_str_radix(amount, 10).map_err(|_| QualificationError::InvalidIntent)?;
        if expected.to_string() != *amount {
            return Err(QualificationError::InvalidIntent);
        }
        Ok((slot, expected))
    }
    fn matches(&self, value: U256, expected: U256) -> bool {
        match self {
            Self::Allowance { .. }
            | Self::TokenBalance {
                comparison: PredicateComparison::Equal,
                ..
            } => value == expected,
            Self::TokenBalance {
                comparison: PredicateComparison::AtLeast,
                ..
            } => value >= expected,
            Self::TokenBalance {
                comparison: PredicateComparison::AtMost,
                ..
            } => value <= expected,
        }
    }
}
/// One proof-checked value; only the opaque enclosing certificate is authoritative.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PredicateObservation {
    predicate: ContractPredicate,
    value: String,
}
impl PredicateObservation {
    /// Exact requested assertion.
    #[must_use]
    pub const fn predicate(&self) -> &ContractPredicate {
        &self.predicate
    }
    /// Exact proved raw unsigned decimal value.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }
}
/// Non-deserialisable certificate binding successful transaction inclusion to
/// fresh canonical code and all requested goal predicates. No persisted digest
/// can recreate this capability after restart.
pub struct VerifiedReceiptPredicates {
    observed_at: Instant,
    candidate: String,
    network: EvmNetwork,
    account: String,
    target: String,
    transaction_hash: String,
    receipt_number: u64,
    receipt_hash: String,
    state_number: u64,
    state_hash: String,
    receipt_commitment: String,
    proof_commitment: String,
    observations: Vec<PredicateObservation>,
}
impl VerifiedReceiptPredicates {
    /// Monotonic process freshness; durable records cannot recreate this proof.
    #[must_use]
    pub fn is_fresh(&self) -> bool {
        self.observed_at.elapsed() < Duration::from_mins(2)
    }
    /// Exact qualified candidate being reconciled.
    #[must_use]
    pub fn candidate_digest(&self) -> &str {
        &self.candidate
    }
    /// Exact network.
    #[must_use]
    pub const fn network(&self) -> EvmNetwork {
        self.network
    }
    /// Exact original caller.
    #[must_use]
    pub fn account(&self) -> &str {
        &self.account
    }
    /// Exact qualified token.
    #[must_use]
    pub fn target(&self) -> &str {
        &self.target
    }
    /// Exact included signed transaction hash.
    #[must_use]
    pub fn transaction_hash(&self) -> &str {
        &self.transaction_hash
    }
    /// Canonical successful receipt height.
    #[must_use]
    pub const fn receipt_block_number(&self) -> u64 {
        self.receipt_number
    }
    /// Canonical successful receipt identity.
    #[must_use]
    pub fn receipt_block_hash(&self) -> &str {
        &self.receipt_hash
    }
    /// Independently proved goal-state height.
    #[must_use]
    pub const fn state_block_number(&self) -> u64 {
        self.state_number
    }
    /// Independently proved goal-state identity.
    #[must_use]
    pub fn state_block_hash(&self) -> &str {
        &self.state_hash
    }
    /// Commitment to verified transaction/receipt inclusions.
    #[must_use]
    pub fn receipt_commitment(&self) -> &str {
        &self.receipt_commitment
    }
    /// Commitment to canonical state/profile/predicate observations.
    #[must_use]
    pub fn proof_commitment(&self) -> &str {
        &self.proof_commitment
    }
    /// Complete ordered goal observations, all satisfied.
    #[must_use]
    pub fn observations(&self) -> &[PredicateObservation] {
        &self.observations
    }
    /// Exact compiled source profile identity.
    #[must_use]
    pub const fn profile_id(&self) -> &str {
        profile::ID
    }
    /// Exact source/compiler/settings commitment.
    #[must_use]
    pub const fn profile_commitment(&self) -> &str {
        profile::REVISION
    }
}

/// Proves exact successful transaction inclusion and independently reads fresh
/// canonical token predicates. Native outbound permission is required before
/// calling. Untrusted proof material and predicates cannot mint a result alone.
///
/// # Errors
/// Rejects mismatched bodies/signers/hashes, failed receipts, invalid proofs,
/// changed source/deployment, reorgs, unmet predicates and resource limits.
pub fn verify_contract_receipt_predicates(
    endpoint: &EvmRpcUrl,
    candidate: &QualifiedContractCandidate,
    transaction_hash: &str,
    inclusion: &DeploymentProof,
    predicates: &[ContractPredicate],
    cancelled: &AtomicBool,
) -> Result<VerifiedReceiptPredicates, QualificationError> {
    if predicates.is_empty() || predicates.len() > 16 {
        return Err(QualificationError::InvalidIntent);
    }
    let control = SimulationControl::start(cancelled);
    let network = candidate.intent().network();
    let receipt_header =
        local_simulation::canonical_header(endpoint, network, &inclusion.block_hash, &control)?;
    verify_inclusion(
        &receipt_header,
        candidate.payload(),
        candidate.intent().account(),
        transaction_hash,
        inclusion,
    )?;
    if receipt_header.number < candidate.block_number() {
        return Err(QualificationError::SnapshotMismatch);
    }
    let target = parse_address(candidate.identity().target())?;
    let words: Vec<_> = predicates
        .iter()
        .map(ContractPredicate::word)
        .collect::<Result<_, _>>()?;
    let queries: Vec<_> = words
        .iter()
        .map(|(slot, _)| StateQuery::Storage {
            address: format!("{target:#x}"),
            slot: format!("{slot:#066x}"),
        })
        .collect();
    let latest = local_simulation::latest_header(endpoint, network, &control)?;
    let state_hash = format!("{:#x}", latest.hash_slow());
    let state_block_hash = state_hash.as_str();
    let (state_number, state) =
        local_simulation::read_proved(endpoint, network, state_block_hash, &queries, &control)?;
    if state_number < receipt_header.number {
        return Err(QualificationError::SnapshotMismatch);
    }
    let account = state
        .account(target)
        .ok_or(QualificationError::MissingState)?;
    if !account.exists() || account.nonce() != 1 {
        return Err(QualificationError::UnsupportedProfile);
    }
    profile::verify(account.code())?;
    let mut observations = Vec::with_capacity(predicates.len());
    for (predicate, (slot, expected)) in predicates.iter().zip(words) {
        let value = state
            .slot(target, slot)
            .ok_or(QualificationError::MissingState)?;
        if !predicate.matches(value, expected) {
            return Err(QualificationError::UncoveredEffects);
        }
        observations.push(PredicateObservation {
            predicate: predicate.clone(),
            value: value.to_string(),
        });
    }
    // Canonicality is rechecked after every state read, including old deployment.
    local_simulation::canonical_header(
        endpoint,
        network,
        &candidate.identity().deployment_block_hash(),
        &control,
    )?;
    local_simulation::canonical_header(endpoint, network, &inclusion.block_hash, &control)?;
    local_simulation::canonical_header(endpoint, network, state_block_hash, &control)?;
    if local_simulation::latest_header(endpoint, network, &control)?.hash_slow()
        != latest.hash_slow()
    {
        return Err(QualificationError::SnapshotMismatch);
    }
    let receipt_commitment = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(inclusion).map_err(|_| QualificationError::InvalidIntent)?
        )
    );
    let proof_commitment=format!("{:x}",Sha256::digest(serde_json::json!({"candidate":candidate.candidate_digest(),"stateBlock":state_block_hash,"profile":profile::REVISION,"observations":observations}).to_string().as_bytes()));
    Ok(VerifiedReceiptPredicates {
        observed_at: Instant::now(),
        candidate: candidate.candidate_digest().into(),
        network,
        account: candidate.intent().account().into(),
        target: format!("{target:#x}"),
        transaction_hash: transaction_hash.into(),
        receipt_number: receipt_header.number,
        receipt_hash: inclusion.block_hash.clone(),
        state_number,
        state_hash: state_block_hash.into(),
        receipt_commitment,
        proof_commitment,
        observations,
    })
}
fn verify_inclusion(
    header: &Header,
    payload: &[u8],
    account: &str,
    expected_hash: &str,
    proof: &DeploymentProof,
) -> Result<(), QualificationError> {
    let reject = QualificationError::SnapshotMismatch;
    let transaction = deployment::hex(&proof.transaction, 8192, false)?;
    let expected = B256::from_str(expected_hash).map_err(|_| reject)?;
    if header.hash_slow() != B256::from_str(&proof.block_hash).map_err(|_| reject)?
        || keccak256(&transaction) != expected
    {
        return Err(reject);
    }
    let mut remaining = transaction.as_slice();
    let signed = Signed::<TxEip1559>::eip2718_decode(&mut remaining).map_err(|_| reject)?;
    if !remaining.is_empty()
        || {
            let mut canonical = Vec::new();
            signed.eip2718_encode(&mut canonical);
            canonical != transaction
        }
        || signed.signature().normalize_s().is_some()
        || signed.tx().encoded_for_signing() != payload
        || signed.recover_signer().map_err(|_| reject)?
            != Address::from_str(account).map_err(|_| reject)?
    {
        return Err(reject);
    }
    let receipt = deployment::hex(&proof.receipt, 4096, false)?;
    let mut remaining = receipt.strip_prefix(&[2]).ok_or(reject)?;
    let decoded = ReceiptWithBloom::<Receipt>::decode(&mut remaining).map_err(|_| reject)?;
    let mut canonical = vec![2];
    canonical.extend(alloy_rlp::encode(&decoded));
    if !remaining.is_empty()
        || canonical != receipt
        || decoded.receipt.status != Eip658Value::Eip658(true)
        || decoded.receipt.cumulative_gas_used == 0
        || decoded.receipt.cumulative_gas_used > header.gas_used
        || decoded.logs_bloom != decoded.receipt.bloom_slow()
    {
        return Err(reject);
    }
    let key = Nibbles::unpack(alloy_rlp::encode(proof.transaction_index));
    verify_proof(
        header.transactions_root,
        key,
        Some(transaction),
        &deployment::proof_nodes(&proof.transaction_proof)?,
    )
    .map_err(|_| reject)?;
    verify_proof(
        header.receipts_root,
        key,
        Some(receipt),
        &deployment::proof_nodes(&proof.receipt_proof)?,
    )
    .map_err(|_| reject)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Signature, TxKind};
    use alloy_trie::nodes::LeafNode;
    fn inclusion(index: u64, bytes: &[u8]) -> (B256, Vec<String>) {
        let node = alloy_rlp::encode(LeafNode::new(
            Nibbles::unpack(alloy_rlp::encode(index)),
            bytes.to_vec(),
        ));
        (
            keccak256(&node),
            vec![format!("0x{}", alloy_primitives::hex::encode(node))],
        )
    }
    fn fixture(
        index: u64,
    ) -> Result<(Header, Vec<u8>, String, String, DeploymentProof), QualificationError> {
        let tx = TxEip1559 {
            chain_id: 1,
            nonce: 3,
            gas_limit: 100_000,
            max_fee_per_gas: 10,
            max_priority_fee_per_gas: 1,
            to: TxKind::Call(Address::repeat_byte(2)),
            ..Default::default()
        };
        let payload = tx.encoded_for_signing();
        let sig = Signature::from_scalars_and_parity(
            "0x79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
                .parse()
                .map_err(|_| QualificationError::InvalidIntent)?,
            B256::from(U256::from(1).to_be_bytes::<32>()),
            false,
        );
        let signed = Signed::new_unhashed(tx, sig);
        let account = format!(
            "{:#x}",
            signed
                .recover_signer()
                .map_err(|_| QualificationError::InvalidIntent)?
        );
        let mut tx = Vec::new();
        signed.eip2718_encode(&mut tx);
        let hash = format!("{:#x}", keccak256(&tx));
        let mut receipt = vec![2];
        receipt.extend(alloy_rlp::encode(
            Receipt {
                status: Eip658Value::Eip658(true),
                cumulative_gas_used: 50_000,
                logs: Vec::<alloy_primitives::Log>::new(),
            }
            .with_bloom(),
        ));
        let (transactions_root, transaction_proof) = inclusion(index, &tx);
        let (receipts_root, receipt_proof) = inclusion(index, &receipt);
        let header = Header {
            transactions_root,
            receipts_root,
            number: 100,
            gas_used: 50_000,
            ..Default::default()
        };
        let proof = DeploymentProof {
            block_hash: format!("{:#x}", header.hash_slow()),
            transaction_index: index,
            transaction: format!("0x{}", alloy_primitives::hex::encode(tx)),
            transaction_proof,
            receipt: format!("0x{}", alloy_primitives::hex::encode(receipt)),
            receipt_proof,
        };
        Ok((header, payload, account, hash, proof))
    }
    #[test]
    fn exact_body_signer_and_both_inclusions_required() -> Result<(), QualificationError> {
        for index in [0, 1, 127, 128, 65_536] {
            let (header, payload, account, hash, proof) = fixture(index)?;
            verify_inclusion(&header, &payload, &account, &hash, &proof)?;
            assert!(
                verify_inclusion(
                    &header,
                    &payload,
                    "0x1111111111111111111111111111111111111111",
                    &hash,
                    &proof
                )
                .is_err()
            );
            assert!(
                verify_inclusion(
                    &header,
                    &payload,
                    &account,
                    &format!("{:#x}", B256::ZERO),
                    &proof
                )
                .is_err()
            );
            let mut changed = payload.clone();
            changed.push(0);
            assert!(verify_inclusion(&header, &changed, &account, &hash, &proof).is_err());
            for mutation in 0..6 {
                let mut changed = proof.clone();
                match mutation {
                    0 => changed.transaction_index += 1,
                    1 => changed.transaction.push_str("00"),
                    2 => changed.receipt.push_str("00"),
                    3 => changed.receipt_proof.clear(),
                    4 => changed.transaction_proof.clear(),
                    _ => changed.block_hash = format!("{:#x}", B256::ZERO),
                }
                assert!(verify_inclusion(&header, &payload, &account, &hash, &changed).is_err());
            }
        }
        Ok(())
    }
    #[test]
    fn failed_receipt_rejects_even_with_valid_receipt_root() -> Result<(), QualificationError> {
        let (mut header, payload, account, hash, mut proof) = fixture(0)?;
        let mut receipt = vec![2];
        receipt.extend(alloy_rlp::encode(
            Receipt {
                status: Eip658Value::Eip658(false),
                cumulative_gas_used: 50_000,
                logs: Vec::<alloy_primitives::Log>::new(),
            }
            .with_bloom(),
        ));
        (header.receipts_root, proof.receipt_proof) = inclusion(0, &receipt);
        proof.receipt = format!("0x{}", alloy_primitives::hex::encode(receipt));
        proof.block_hash = format!("{:#x}", header.hash_slow());
        assert!(verify_inclusion(&header, &payload, &account, &hash, &proof).is_err());
        Ok(())
    }
    #[test]
    fn assertions_require_canonical_values_and_known_semantic_slots() {
        let p = ContractPredicate::Allowance {
            owner: format!("{:#x}", Address::repeat_byte(1)),
            spender: format!("{:#x}", Address::repeat_byte(2)),
            amount: "3".into(),
        };
        assert_eq!(
            p.word().map(|v| v.0),
            Ok(allowance_slot(
                Address::repeat_byte(1),
                Address::repeat_byte(2)
            ))
        );
        assert!(!p.matches(U256::from(4), U256::from(3)));
        for amount in ["03", "-1", "0x3", ""] {
            let p = ContractPredicate::TokenBalance {
                account: format!("{:#x}", Address::repeat_byte(1)),
                comparison: PredicateComparison::AtLeast,
                amount: amount.into(),
            };
            assert!(p.word().is_err());
        }
        for comparison in [
            PredicateComparison::Equal,
            PredicateComparison::AtLeast,
            PredicateComparison::AtMost,
        ] {
            let p = ContractPredicate::TokenBalance {
                account: format!("{:#x}", Address::repeat_byte(1)),
                comparison,
                amount: "3".into(),
            };
            assert!(p.matches(U256::from(3), U256::from(3)));
            assert_eq!(
                p.matches(U256::from(4), U256::from(3)),
                comparison == PredicateComparison::AtLeast
            );
            assert_eq!(
                p.matches(U256::from(2), U256::from(3)),
                comparison == PredicateComparison::AtMost
            );
        }
    }
}
