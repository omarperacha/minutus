use super::{
    ContractAction, DeploymentProof, QualificationError, QualifiedContractIdentity,
    QualifiedEffects, deployment::VerifiedDeployment,
};
use crate::{
    DecodedEip1559Transaction, EvmNetwork, EvmRpcUrl,
    contract::{ContractBlock, UnsignedContractIntent},
    local_simulation::{self, ProvedSimulation, SimulationFeeFacts},
};
use evm_simulation::{SimulationControl, TrustedSimulationWorker};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::atomic::AtomicBool;

/// Immutable independently decoded exact candidate with sealed source/effect
/// evidence. Not serialisable, not approval and never a direct signing handle.
pub struct QualifiedWriteCandidate {
    intent: UnsignedContractIntent,
    identity: QualifiedContractIdentity,
    effects: QualifiedEffects,
    run: ProvedSimulation,
    payload: Vec<u8>,
    decoded: DecodedEip1559Transaction,
    commitment: String,
    semantic_commitment: String,
    effect_commitment: String,
}
impl QualifiedWriteCandidate {
    fn from_run(
        intent: UnsignedContractIntent,
        run: ProvedSimulation,
        deployment: VerifiedDeployment,
    ) -> Result<Self, QualificationError> {
        let req = run.request();
        let identity = QualifiedContractIdentity::from_run(&run, deployment)?;
        let effects = QualifiedEffects::from_run(&run, ContractAction::parse(&intent)?)?;
        let (payload, decoded) = super::envelope::prepare(&intent, req)?;
        let semantic_commitment = digest(
            &json!({"chain":decoded.caip2_id(),"account":intent.account(),"target":intent.call().target(),"function":intent.call().signature(),"arguments":intent.call().arguments(),"value":"0","profile":identity.profile_id(),"revision":identity.profile_revision()}),
        );
        let effect_commitment = digest(
            &json!({"action":effects.action(),"balances":effects.balances(),"allowance":effects.allowance(),"executionFee":effects.execution_fee_wei(),"maximumExecutionFee":effects.maximum_execution_fee_wei()}),
        );
        let commitment = format!(
            "0x{}",
            digest(
                &json!({"semantic":semantic_commitment,"effects":effect_commitment,"request":run.observation().request_commitment,"block":req.block.hash,"source":identity.profile_revision(),"deployment":identity.deployment_transaction_hash(),"payload":format!("{:x}",Sha256::digest(&payload))})
            )
        );
        Ok(Self {
            intent,
            identity,
            effects,
            run,
            payload,
            decoded,
            commitment,
            semantic_commitment,
            effect_commitment,
        })
    }
    /// Exact requested public intent, already independently round-tripped.
    #[must_use]
    pub const fn intent(&self) -> &UnsignedContractIntent {
        &self.intent
    }
    /// Source/runtime/creation correspondence; no untrusted metadata label.
    #[must_use]
    pub const fn identity(&self) -> &QualifiedContractIdentity {
        &self.identity
    }
    /// Complete reviewed-profile material effects at this snapshot.
    #[must_use]
    pub const fn effects(&self) -> &QualifiedEffects {
        &self.effects
    }
    /// Independently decoded exact transaction envelope (legacy semantic status
    /// remains separate from the reviewed profile certificate).
    #[must_use]
    pub const fn decoded(&self) -> &DecodedEip1559Transaction {
        &self.decoded
    }
    /// Exact canonical unsigned type-2 payload; no signature/approval capability.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
    /// Exact candidate commitment, including fresh state/fees/nonce and evidence.
    #[must_use]
    pub fn candidate_digest(&self) -> &str {
        &self.commitment
    }
    /// Stable semantic intent, excluding fresh state/nonce/fees.
    #[must_use]
    pub fn semantic_commitment(&self) -> &str {
        &self.semantic_commitment
    }
    /// Exact effect/native-fee certificate commitment, not model-supplied evidence.
    #[must_use]
    pub fn effect_commitment(&self) -> &str {
        &self.effect_commitment
    }
    /// Snapshot block height.
    #[must_use]
    pub fn block_number(&self) -> u64 {
        self.run.request().block.number
    }
    /// Native gas used by this exact local execution, distinct from the signed cap.
    #[must_use]
    pub fn gas_used(&self) -> u64 {
        self.run.observation().gas_used
    }
}

/// Prepares one reviewed direct-profile Ethereum candidate from proof-checked RPC
/// state and a trusted host-selected isolated worker. The caller must obtain the
/// exact native outbound permit before this API; this grants no spend approval.
///
/// # Errors
/// Rejects unsupported code/proxy/function, missing canonical creation proofs,
/// incomplete execution/effects, state faults, or an unqualified chain protocol.
pub fn prepare_qualified_contract_with_worker(
    worker: &TrustedSimulationWorker,
    endpoint: &EvmRpcUrl,
    intent: &UnsignedContractIntent,
    block: &ContractBlock,
    fees: SimulationFeeFacts,
    deployment: &DeploymentProof,
    cancelled: &AtomicBool,
) -> Result<QualifiedWriteCandidate, QualificationError> {
    if !matches!(
        intent.network(),
        EvmNetwork::EthereumMainnet | EvmNetwork::EthereumSepolia
    ) {
        return Err(QualificationError::UnsupportedNetwork);
    }
    let action = ContractAction::parse(intent)?;
    let target = super::action::parse_address(intent.call().target())?;
    let control = SimulationControl::start(cancelled);
    let header = local_simulation::canonical_header(
        endpoint,
        intent.network(),
        &deployment.block_hash,
        &control,
    )?;
    let verified =
        VerifiedDeployment::verify_against_header(&header, intent.network(), target, deployment)?;
    let queries = action.queries(target)?;
    let run =
        local_simulation::run_proved(worker, endpoint, intent, block, fees, &control, &queries)?;
    if header.number > run.request().block.number {
        return Err(QualificationError::InvalidDeployment);
    }
    local_simulation::canonical_header(
        endpoint,
        intent.network(),
        &deployment.block_hash,
        &control,
    )?;
    QualifiedWriteCandidate::from_run(intent.clone(), run, verified)
}
fn digest(value: &serde_json::Value) -> String {
    format!("{:x}", Sha256::digest(value.to_string().as_bytes()))
}
