//! Immutable generic candidate, constructed only from trusted proved replay.
use super::{
    abi,
    effect_envelope::GenericEffectEnvelope,
    effects::{
        GenericEffects, GenericNativeAccountMode, GenericNativeTransfer, NativeEffectContext,
    },
    envelope,
    error::GenericError,
    identity::GenericContractIdentity,
};
use crate::{
    DecodedEip1559Transaction, contract::UnsignedContractIntent, local_simulation::TrustedReplay,
};
use alloy_primitives::{Address, U256, keccak256};
use sha2::{Digest, Sha256};
use solidity_compilation::ReproducedArtifact;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

const MAX_AGE: Duration = Duration::from_secs(30);

/// Exact source, bounded complete effects and unsigned canonical body. This
/// capability is not serialisable, cloneable, a native approval or a signer.
pub struct GenericContractCandidate {
    intent: UnsignedContractIntent,
    identity: GenericContractIdentity,
    effects: GenericEffects,
    envelope: GenericEffectEnvelope,
    replay: TrustedReplay,
    payload: Vec<u8>,
    decoded: DecodedEip1559Transaction,
    candidate_digest: String,
    semantic_commitment: String,
    effect_commitment: String,
    maximum_fee_wei: String,
    maximum_total_debit_wei: String,
    prepared_at: Instant,
    business_scope: Vec<String>,
}
impl GenericContractCandidate {
    #[cfg(test)]
    pub(crate) fn from_replay(
        intent: UnsignedContractIntent,
        replay: TrustedReplay,
        artifacts: &[(Address, Arc<ReproducedArtifact>)],
    ) -> Result<Self, GenericError> {
        let business = vec![intent.call().target().to_owned()];
        Self::from_replay_in_scope(intent, replay, artifacts, &business)
    }
    pub(crate) fn from_replay_in_scope(
        intent: UnsignedContractIntent,
        replay: TrustedReplay,
        artifacts: &[(Address, Arc<ReproducedArtifact>)],
        business: &[String],
    ) -> Result<Self, GenericError> {
        let prepared_at = replay.prepared_at();
        let request = replay.request();
        let entry: Address = intent
            .call()
            .target()
            .parse()
            .map_err(|_| GenericError::SnapshotMismatch)?;
        let identity = GenericContractIdentity::from_scoped_source_state(
            intent.network(),
            entry,
            replay.state(),
            artifacts,
            business,
        )?;
        // Construct the source proof before consulting the observed call tree.
        let envelope = super::effect_envelope::graph_admit(&identity, &intent, business)?;
        let actual = GenericContractIdentity::from_state(
            intent.network(),
            entry,
            replay.state(),
            artifacts,
            &replay.observation().calls,
            replay.calls(),
            replay.written_slots(),
        )?;
        identity.verify_execution_graph(&actual)?;
        verify_selected_source(&intent, &identity, &replay, artifacts)?;
        let preimages = material_preimages(&replay, &envelope, &identity)?;
        let transfers = native_transfers(&replay)?;
        if intent.network().is_base() && (!replay.is_base_material() || !transfers.is_empty()) {
            return Err(GenericError::ExecutionRejected);
        }
        let effects = GenericEffects::from_observation(
            &intent,
            &identity,
            replay.state(),
            replay.observation(),
            &preimages,
            NativeEffectContext {
                transfers,
                mode: if intent.network().is_base() {
                    GenericNativeAccountMode::ConservativeBasePrepayment
                } else {
                    GenericNativeAccountMode::ExactTransaction
                },
            },
            replay.calls(),
        )?;
        envelope.verify(
            &intent,
            &effects,
            &identity,
            replay.state(),
            replay.written_slots(),
        )?;
        let (payload, decoded) = envelope::prepare(&intent, request)?;
        let (fee, total) = budget(&intent, &replay)?;
        let semantic_commitment = digest(
            &serde_json::json!({"domain":"minutus-generic-intent-v1","network":intent.network().descriptor().chain_id(),"account":intent.account(),"target":intent.call().target(),"function":intent.call().signature(),"arguments":intent.call().arguments(),"value":intent.value_wei(),"source":identity.commitment()}),
        )?;
        let effect_commitment = digest(
            &serde_json::json!({"domain":"minutus-generic-effects-v1","material":effects.material_commitment(),"envelope":envelope.commitment(),"maximumFee":fee.to_string(),"maximumDebit":total.to_string()}),
        )?;
        let candidate_digest = format!(
            "0x{}",
            digest(
                &serde_json::json!({"domain":"minutus-generic-candidate-v1","semantic":semantic_commitment,"effects":effect_commitment,"block":request.block.hash,"height":request.block.number,"payload":format!("{:x}",Sha256::digest(&payload))})
            )?
        );
        Ok(Self {
            intent,
            identity,
            effects,
            envelope,
            replay,
            payload,
            decoded,
            candidate_digest,
            semantic_commitment,
            effect_commitment,
            maximum_fee_wei: fee.to_string(),
            maximum_total_debit_wei: total.to_string(),
            prepared_at,
            business_scope: {
                let mut scope = business.to_vec();
                scope.sort();
                scope
            },
        })
    }
    /// Exact canonical invocation selected by the user-scoped workflow.
    #[must_use]
    pub const fn intent(&self) -> &UnsignedContractIntent {
        &self.intent
    }
    /// Current code/source and actual bounded delegation correspondence.
    #[must_use]
    pub const fn identity(&self) -> &GenericContractIdentity {
        &self.identity
    }
    /// Complete typed material effects and independent native-account accounting.
    #[must_use]
    pub const fn effects(&self) -> &GenericEffects {
        &self.effects
    }
    /// Onchain-enforced bounded source-field relations, separate from snapshot
    /// predictions and never a universal economic/asset interpretation.
    #[must_use]
    pub const fn envelope(&self) -> &GenericEffectEnvelope {
        &self.envelope
    }
    /// Exact business-address scope retained by refresh; not a native approval
    /// or permission to acquire another undeclared target.
    #[must_use]
    pub fn business_scope(&self) -> &[String] {
        &self.business_scope
    }
    /// Independently decoded canonical type-2 transaction facts.
    #[must_use]
    pub const fn decoded(&self) -> &DecodedEip1559Transaction {
        &self.decoded
    }
    /// Canonical unsigned body, carrying no signature or approval authority.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
    /// Exact fresh candidate binding in canonical 0x-prefixed hash form.
    #[must_use]
    pub fn candidate_digest(&self) -> &str {
        &self.candidate_digest
    }
    /// Semantic invocation and source binding, excluding fresh nonce/fees/state.
    #[must_use]
    pub fn semantic_commitment(&self) -> &str {
        &self.semantic_commitment
    }
    /// Exact application material and financial-budget binding.
    #[must_use]
    pub fn effect_commitment(&self) -> &str {
        &self.effect_commitment
    }
    /// Complete source/code/control-state identity.
    #[must_use]
    pub fn source_commitment(&self) -> &str {
        self.identity.commitment()
    }
    /// Maximum protocol fee at this snapshot, decimal wei.
    #[must_use]
    pub fn maximum_fee_wei(&self) -> &str {
        &self.maximum_fee_wei
    }
    /// Maximum fee plus native transaction value debited from the selected EOA.
    #[must_use]
    pub fn maximum_total_debit_wei(&self) -> &str {
        &self.maximum_total_debit_wei
    }
    /// Exact canonical state height of this replay.
    #[must_use]
    pub fn block_number(&self) -> u64 {
        self.replay.request().block.number
    }
    /// Exact canonical state block hash.
    #[must_use]
    pub fn block_hash(&self) -> &str {
        &self.replay.request().block.hash
    }
    /// Bounded monotonic evidence lifetime; native scope checks remain separate.
    #[must_use]
    pub fn is_fresh(&self) -> bool {
        self.prepared_at.elapsed() < MAX_AGE
    }
    /// Trusted owner data, unavailable through model or serialised projections.
    pub(crate) const fn replay(&self) -> &TrustedReplay {
        &self.replay
    }
    /// Consumes candidate ownership for the closed signed Base validation phase.
    pub(crate) fn into_replay(self) -> TrustedReplay {
        self.replay
    }
}
fn verify_selected_source(
    intent: &UnsignedContractIntent,
    identity: &GenericContractIdentity,
    replay: &TrustedReplay,
    artifacts: &[(Address, Arc<ReproducedArtifact>)],
) -> Result<(), GenericError> {
    let entry = intent
        .call()
        .target()
        .parse()
        .map_err(|_| GenericError::InvalidAbi)?;
    let artifact = identity.entry_artifact()?;
    if let Some(value) = replay.state().slot(entry, super::proxy::IMPLEMENTATION)
        && value != U256::ZERO
    {
        let potential = super::proxy::slot_address(value)?;
        let (_, expected) = artifacts
            .iter()
            .find(|(address, _)| *address == potential)
            .ok_or(GenericError::SourceMismatch)?;
        let proved = replay
            .state()
            .account(potential)
            .ok_or(GenericError::MissingState)?;
        if expected.runtime() != proved.code() {
            return Err(GenericError::SourceMismatch);
        }
        if expected.methods().get(intent.call().signature()).is_some()
            && expected.commitment() != artifact.commitment()
        {
            return Err(GenericError::UnsupportedProxy);
        }
    }
    abi::bind(artifact.abi(), intent)?;
    let selector =
        alloy_primitives::hex::encode(&keccak256(intent.call().signature().as_bytes())[..4]);
    if artifact.methods()[intent.call().signature()] != selector {
        return Err(GenericError::InvalidAbi);
    }
    Ok(())
}

fn material_preimages(
    replay: &TrustedReplay,
    envelope: &GenericEffectEnvelope,
    identity: &GenericContractIdentity,
) -> Result<Vec<Vec<u8>>, GenericError> {
    let mut preimages = replay.keccak_preimages().to_vec();
    // Optimiser-folded constant mapping hashes have no KECCAK trace entry.
    // Only the opaque compiler-AST envelope may supply their canonical paths.
    for input in envelope.mapping_preimages(identity)? {
        if !preimages.contains(&input) {
            preimages.push(input);
        }
    }
    Ok(preimages)
}
fn native_transfers(replay: &TrustedReplay) -> Result<Vec<GenericNativeTransfer>, GenericError> {
    replay
        .native_transfers()
        .iter()
        .map(|transfer| {
            let from: Address = transfer
                .from()
                .parse()
                .map_err(|_| GenericError::SnapshotMismatch)?;
            let to: Address = transfer
                .to()
                .parse()
                .map_err(|_| GenericError::SnapshotMismatch)?;
            Ok(GenericNativeTransfer {
                from: format!("{from:#x}"),
                to: format!("{to:#x}"),
                value_wei: amount(transfer.value())?.to_string(),
            })
        })
        .collect()
}
fn budget(
    intent: &UnsignedContractIntent,
    replay: &TrustedReplay,
) -> Result<(U256, U256), GenericError> {
    let request = replay.request();
    let fee = if intent.network().is_base() {
        amount(
            replay
                .base_fee_ceiling()
                .ok_or(GenericError::ExecutionRejected)?,
        )?
    } else {
        U256::from(request.transaction.gas_limit)
            .checked_mul(U256::from(request.transaction.max_fee_per_gas))
            .ok_or(GenericError::Limit)?
    };
    let total = fee
        .checked_add(amount(intent.value_wei())?)
        .ok_or(GenericError::Limit)?;
    let sender: Address = intent
        .account()
        .parse()
        .map_err(|_| GenericError::SnapshotMismatch)?;
    if replay.state().account(sender).is_none_or(|account| {
        account.balance() < total
            || account.nonce() != request.transaction.nonce
            || !account.code().is_empty()
    }) {
        return Err(GenericError::SnapshotMismatch);
    }
    Ok((fee, total))
}
fn amount(value: &str) -> Result<U256, GenericError> {
    if let Some(hex) = value.strip_prefix("0x") {
        U256::from_str_radix(hex, 16)
    } else {
        U256::from_str_radix(value, 10)
    }
    .map_err(|_| GenericError::SnapshotMismatch)
}
fn digest(value: &serde_json::Value) -> Result<String, GenericError> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).map_err(|_| GenericError::SnapshotMismatch)?)
    ))
}
