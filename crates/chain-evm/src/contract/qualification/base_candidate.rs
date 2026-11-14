//! Base preapproval source/material certificate and closed post-sign execution.
//! Exact native execution is intentionally deferred until the genuine envelope
//! exists; unsigned material semantics and rigorous financial ceilings precede it.
use super::{
    ContractAction, DeploymentProof, QualificationError, QualifiedContractIdentity,
    QualifiedMaterialEffects, action::parse_address, deployment::VerifiedDeployment,
};
use crate::{
    DecodedEip1559Transaction, EvmNetwork, EvmRpcUrl,
    contract::{ContractBlock, UnsignedContractIntent},
    local_simulation::{self, ProvedBaseSnapshot, SimulationFeeFacts},
};
use alloy_primitives::U256;
use evm_simulation::{
    SimulationControl, TrustedSimulationWorker,
    offline::{ValidatedOfflineExecution, execute_base_offline},
};
use sha2::{Digest, Sha256};
use std::sync::atomic::AtomicBool;

/// Exact unsigned Base body, source-derived material effects and all-signature
/// fee ceiling. This does not claim full native execution before signing.
pub struct QualifiedBaseWriteCandidate {
    intent: UnsignedContractIntent,
    identity: QualifiedContractIdentity,
    material: QualifiedMaterialEffects,
    snapshot: ProvedBaseSnapshot,
    payload: Vec<u8>,
    decoded: DecodedEip1559Transaction,
    maximum_fee: String,
    commitment: String,
    semantic_commitment: String,
    effect_commitment: String,
}
impl QualifiedBaseWriteCandidate {
    /// Immutable original semantic intent.
    #[must_use]
    pub const fn intent(&self) -> &UnsignedContractIntent {
        &self.intent
    }
    /// Exact source/runtime/canonical deployment correspondence.
    #[must_use]
    pub const fn identity(&self) -> &QualifiedContractIdentity {
        &self.identity
    }
    /// Source-derived fee-insensitive material changes, excluding native settlement.
    #[must_use]
    pub const fn material(&self) -> &QualifiedMaterialEffects {
        &self.material
    }
    /// Independently decoded complete unsigned transaction.
    #[must_use]
    pub const fn decoded(&self) -> &DecodedEip1559Transaction {
        &self.decoded
    }
    /// Exact signing bytes; no approval or signature capability.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
    /// Maximum total native fee at the frozen proof snapshot for every signature.
    #[must_use]
    pub fn maximum_fee_wei(&self) -> &str {
        &self.maximum_fee
    }
    /// Exact candidate, frozen state, material and financial commitment.
    #[must_use]
    pub fn candidate_digest(&self) -> &str {
        &self.commitment
    }
    /// Stable semantic intent, without fresh nonce/fee/state fields.
    #[must_use]
    pub fn semantic_commitment(&self) -> &str {
        &self.semantic_commitment
    }
    /// Exact material effect certificate commitment.
    #[must_use]
    pub fn effect_commitment(&self) -> &str {
        &self.effect_commitment
    }
    /// Exact frozen state commitment; not authority after restart.
    #[must_use]
    pub fn snapshot_commitment(&self) -> &str {
        self.snapshot.frozen().commitment()
    }
    /// Snapshot height.
    #[must_use]
    pub fn block_number(&self) -> u64 {
        self.snapshot.request().block.number
    }
    /// Consumes frozen state in an attested strictly offline native Base worker.
    /// No source, broker or endpoint is available after signature material enters.
    ///
    /// # Errors
    /// Rejects wrong body/signer, unsupported/missing state, reverts, extra effects,
    /// changed native settlement, cancellation, limits or helper identity failure.
    pub(super) fn validate_signed(
        self,
        worker: &TrustedSimulationWorker,
        signed: &[u8],
        cancelled: &AtomicBool,
    ) -> Result<ValidatedOfflineExecution, QualificationError> {
        let control = SimulationControl::start(cancelled);
        let run = execute_base_offline(worker, self.snapshot.into_frozen(), signed, &control)?;
        Ok(run.validate(self.material.offline_expectation())?)
    }
}
/// Establishes exact source/material correspondence and a rigorous native fee
/// ceiling from a fully frozen proof manifest before any signature exists.
/// Native outbound scope permission is required before invoking this API.
///
/// # Errors
/// Unknown source/proxy/functions, missing deployment/proofs/fee state, unavailable
/// chain rules, insufficient funds, nonzero value and arithmetic faults reject.
pub fn prepare_qualified_base_contract(
    endpoint: &EvmRpcUrl,
    intent: &UnsignedContractIntent,
    block: &ContractBlock,
    fees: SimulationFeeFacts,
    deployment: &DeploymentProof,
    cancelled: &AtomicBool,
) -> Result<QualifiedBaseWriteCandidate, QualificationError> {
    if !matches!(intent.network(), EvmNetwork::Mainnet | EvmNetwork::Sepolia) {
        return Err(QualificationError::UnsupportedNetwork);
    }
    let action = ContractAction::parse(intent)?;
    let target = parse_address(intent.call().target())?;
    let control = SimulationControl::start(cancelled);
    let header = local_simulation::canonical_header(
        endpoint,
        intent.network(),
        &deployment.block_hash,
        &control,
    )?;
    let verified =
        VerifiedDeployment::verify_against_header(&header, intent.network(), target, deployment)?;
    let snapshot = local_simulation::freeze_proved_base(
        endpoint,
        intent,
        block,
        fees,
        &action.queries(target)?,
        &control,
    )?;
    let request = snapshot.request();
    let tx = &request.transaction;
    let identity = QualifiedContractIdentity::from_state(request, snapshot.state(), verified)?;
    let material = QualifiedMaterialEffects::from_state(snapshot.state(), target, action)?;
    let caller = parse_address(intent.account())?;
    let account = snapshot
        .state()
        .account(caller)
        .ok_or(QualificationError::MissingState)?;
    let maximum_fee = U256::from_str_radix(
        snapshot
            .frozen()
            .fee_ceiling()?
            .total_wei()
            .trim_start_matches("0x"),
        16,
    )
    .map_err(|_| QualificationError::Arithmetic)?;
    if !account.exists()
        || !account.code().is_empty()
        || account.nonce() != tx.nonce
        || tx.nonce == u64::MAX
        || account.balance() < maximum_fee
    {
        return Err(QualificationError::SnapshotMismatch);
    }
    let (payload, decoded) = super::envelope::prepare(intent, request)?;
    local_simulation::canonical_header(
        endpoint,
        intent.network(),
        &deployment.block_hash,
        &control,
    )?;
    let semantic_commitment = digest(
        &serde_json::json!({"chain":decoded.caip2_id(),"account":intent.account(),"target":intent.call().target(),"function":intent.call().signature(),"arguments":intent.call().arguments(),"value":"0","profile":identity.profile_id(),"revision":identity.profile_revision()}),
    );
    let effect_commitment = digest(
        &serde_json::json!({"action":material.action(),"balances":material.balances(),"allowance":material.allowance()}),
    );
    let commitment = format!(
        "0x{}",
        digest(
            &serde_json::json!({"mode":"base-source-and-offline-v1","semantic":semantic_commitment,"effects":effect_commitment,"maximumNativeFee":maximum_fee.to_string(),"snapshot":snapshot.frozen().commitment(),"deployment":identity.deployment_transaction_hash(),"payload":format!("{:x}",Sha256::digest(&payload))})
        )
    );
    Ok(QualifiedBaseWriteCandidate {
        intent: intent.clone(),
        identity,
        material,
        snapshot,
        payload,
        decoded,
        maximum_fee: maximum_fee.to_string(),
        commitment,
        semantic_commitment,
        effect_commitment,
    })
}
fn digest(value: &serde_json::Value) -> String {
    format!("{:x}", Sha256::digest(value.to_string().as_bytes()))
}
