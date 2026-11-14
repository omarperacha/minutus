//! Unified host capability; model-visible data and saved JSON cannot construct it.
use super::{
    AllowanceEffect, ContractAction, DeploymentProof, QualificationError,
    QualifiedBaseWriteCandidate, QualifiedContractIdentity, QualifiedWriteCandidate,
    TokenBalanceEffect, prepare_qualified_base_contract, prepare_qualified_contract_with_worker,
};
use crate::{
    DecodedEip1559Transaction, EvmRpcUrl,
    contract::{ContractBlock, UnsignedContractIntent},
    local_simulation::SimulationFeeFacts,
};
use alloy_consensus::{SignableTransaction, Signed, TxEip1559};
use alloy_primitives::{Address, U256, keccak256};
use evm_simulation::{SimulationControl, TrustedSimulationWorkers};
use std::{
    str::FromStr,
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

const MAX_REFRESH_AGE: Duration = Duration::from_secs(30);

/// Complete protocol-specific qualification retained by the trusted host.
pub enum QualifiedContractCandidate {
    /// Exact local Ethereum execution plus source/material/native checks.
    Ethereum(QualifiedWriteCandidate),
    /// Source-derived Base material certificate; full native run follows signing.
    Base(QualifiedBaseWriteCandidate),
}
impl QualifiedContractCandidate {
    /// Exact immutable semantic intent.
    #[must_use]
    pub fn intent(&self) -> &UnsignedContractIntent {
        match self {
            Self::Ethereum(c) => c.intent(),
            Self::Base(c) => c.intent(),
        }
    }
    /// Exact code/source/deployment correspondence.
    #[must_use]
    pub fn identity(&self) -> &QualifiedContractIdentity {
        match self {
            Self::Ethereum(c) => c.identity(),
            Self::Base(c) => c.identity(),
        }
    }
    /// Complete independently decoded transaction body.
    #[must_use]
    pub fn decoded(&self) -> &DecodedEip1559Transaction {
        match self {
            Self::Ethereum(c) => c.decoded(),
            Self::Base(c) => c.decoded(),
        }
    }
    /// Exact unsigned payload, carrying no authority.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        match self {
            Self::Ethereum(c) => c.payload(),
            Self::Base(c) => c.payload(),
        }
    }
    /// Exact candidate/source/material/financial commitment.
    #[must_use]
    pub fn candidate_digest(&self) -> &str {
        match self {
            Self::Ethereum(c) => c.candidate_digest(),
            Self::Base(c) => c.candidate_digest(),
        }
    }
    /// Exact semantic intent commitment.
    #[must_use]
    pub fn semantic_commitment(&self) -> &str {
        match self {
            Self::Ethereum(c) => c.semantic_commitment(),
            Self::Base(c) => c.semantic_commitment(),
        }
    }
    /// Exact protocol-specific evidence commitment.
    #[must_use]
    pub fn effect_commitment(&self) -> &str {
        match self {
            Self::Ethereum(c) => c.effect_commitment(),
            Self::Base(c) => c.effect_commitment(),
        }
    }
    /// Exact preparation height.
    #[must_use]
    pub fn block_number(&self) -> u64 {
        match self {
            Self::Ethereum(c) => c.block_number(),
            Self::Base(c) => c.block_number(),
        }
    }
    /// Source-qualified material action.
    #[must_use]
    pub fn action(&self) -> &ContractAction {
        match self {
            Self::Ethereum(c) => c.effects().action(),
            Self::Base(c) => c.material().action(),
        }
    }
    /// Complete relevant holder changes, without untrusted token labels.
    #[must_use]
    pub fn balances(&self) -> &[TokenBalanceEffect] {
        match self {
            Self::Ethereum(c) => c.effects().balances(),
            Self::Base(c) => c.material().balances(),
        }
    }
    /// Relevant authority change, including unchanged maximum allowance.
    #[must_use]
    pub fn allowance(&self) -> Option<&AllowanceEffect> {
        match self {
            Self::Ethereum(c) => c.effects().allowance(),
            Self::Base(c) => c.material().allowance(),
        }
    }
    /// Maximum total native fee at the prepared snapshot (decimal wei).
    #[must_use]
    pub fn maximum_fee_wei(&self) -> &str {
        match self {
            Self::Ethereum(c) => c.effects().maximum_execution_fee_wei(),
            Self::Base(c) => c.maximum_fee_wei(),
        }
    }
}
/// Revalidation bound to an existing approved body/material/budget. New state
/// never expands the approved financial or token authority.
pub struct RefreshedQualifiedCandidate {
    refreshed_at: Instant,
    approved_digest: String,
    fresh: QualifiedContractCandidate,
}
/// Only the known hash and host-generated evidence IDs leave the signed check.
pub struct ValidatedContractExecution {
    refreshed_at: Instant,
    transaction_hash: String,
    approved_digest: String,
    fresh_digest: String,
}
impl ValidatedContractExecution {
    /// Final submit guards must check this and their own cancellation/scope state.
    /// Signed validation never resets the original refresh's monotonic lifetime.
    #[must_use]
    pub fn is_fresh(&self) -> bool {
        is_fresh(self.refreshed_at)
    }
    /// Independently decoded genuine signed transaction identity.
    #[must_use]
    pub fn transaction_hash(&self) -> &str {
        &self.transaction_hash
    }
    /// Original native-approved evidence identity.
    #[must_use]
    pub fn approved_candidate_digest(&self) -> &str {
        &self.approved_digest
    }
    /// Refreshed, material-equivalent evidence identity.
    #[must_use]
    pub fn fresh_candidate_digest(&self) -> &str {
        &self.fresh_digest
    }
}
impl RefreshedQualifiedCandidate {
    /// Native signing guards must reject expired evidence before using its body.
    #[must_use]
    pub fn is_fresh(&self) -> bool {
        is_fresh(self.refreshed_at)
    }
    /// Latest materially equivalent candidate for final host prerequisite checks.
    #[must_use]
    pub const fn fresh(&self) -> &QualifiedContractCandidate {
        &self.fresh
    }
    /// Validates exact signed body and, for Base, consumes the frozen state in the
    /// strictly offline maintained native handler before any submission.
    ///
    /// # Errors
    /// Any signed-body discrepancy, state fault, extra effect, failed execution,
    /// fee overflow, helper mismatch, cancellation or incomplete material rejects.
    pub fn validate_signed(
        self,
        workers: &TrustedSimulationWorkers,
        signed: &[u8],
        cancelled: &AtomicBool,
    ) -> Result<ValidatedContractExecution, QualificationError> {
        let control = SimulationControl::start(cancelled);
        require_current(self.refreshed_at, &control)?;
        let base = matches!(&self.fresh, QualifiedContractCandidate::Base(_));
        let hash =
            validate_signed_body(self.fresh.payload(), self.fresh.intent().account(), signed)
                .map_err(|error| {
                    if base {
                        QualificationError::IncompleteSimulation
                    } else {
                        error
                    }
                })?;
        let fresh_digest = self.fresh.candidate_digest().to_owned();
        match self.fresh {
            QualifiedContractCandidate::Base(candidate) => validate_base_signed_phase(
                || candidate.validate_signed(workers.offline(), signed, cancelled),
                |checked| checked.transaction_hash() == hash,
                || require_current(self.refreshed_at, &control),
            )?,
            QualifiedContractCandidate::Ethereum(_) => {
                require_current(self.refreshed_at, &control)?;
            }
        }
        Ok(ValidatedContractExecution {
            refreshed_at: self.refreshed_at,
            transaction_hash: hash,
            approved_digest: self.approved_digest,
            fresh_digest,
        })
    }
}
// A child-selected error enum is still signed-input-derived data. No detailed
// Base worker/settlement error, hash discrepancy or child-controlled post-run
// expiry/cancellation variant crosses the native/model release boundary. The
// entry host cancellation check precedes this phase. Physical timing channels
// are not eliminated by this fixed projection.
fn validate_base_signed_phase<T>(
    execute: impl FnOnce() -> Result<T, QualificationError>,
    expected: impl FnOnce(&T) -> bool,
    current: impl FnOnce() -> Result<(), QualificationError>,
) -> Result<(), QualificationError> {
    let checked = execute().map_err(|_| QualificationError::IncompleteSimulation)?;
    if !expected(&checked) {
        return Err(QualificationError::IncompleteSimulation);
    }
    current().map_err(|_| QualificationError::IncompleteSimulation)
}
/// Prepares from a fixed reviewed profile using only packaged attested helpers.
/// The native outbound scope permit must precede all RPC work.
///
/// # Errors
/// Rejects every missing/unsupported/inconsistent source, state, protocol or effect.
pub fn prepare_qualified_contract(
    workers: &TrustedSimulationWorkers,
    endpoint: &EvmRpcUrl,
    intent: &UnsignedContractIntent,
    block: &ContractBlock,
    fees: SimulationFeeFacts,
    deployment: &DeploymentProof,
    cancelled: &AtomicBool,
) -> Result<QualifiedContractCandidate, QualificationError> {
    if intent.network().is_base() {
        prepare_qualified_base_contract(endpoint, intent, block, fees, deployment, cancelled)
            .map(QualifiedContractCandidate::Base)
    } else {
        prepare_qualified_contract_with_worker(
            workers.online(),
            endpoint,
            intent,
            block,
            fees,
            deployment,
            cancelled,
        )
        .map(QualifiedContractCandidate::Ethereum)
    }
}
/// Refreshes current canonical source/state/nonce/fees, preserving the reviewed
/// body and every relevant token/authority effect. New native fees must remain
/// within the original reviewed maximum. Material changes require new approval.
///
/// # Errors
/// Rejects any changed intent/body/nonce/source/deployment/material, insufficient
/// current balance, unsupported rules or a current fee ceiling above the approval.
pub fn refresh_qualified_contract(
    workers: &TrustedSimulationWorkers,
    endpoint: &EvmRpcUrl,
    old: &QualifiedContractCandidate,
    deployment: &DeploymentProof,
    cancelled: &AtomicBool,
) -> Result<RefreshedQualifiedCandidate, QualificationError> {
    let refreshed_at = Instant::now();
    let decoded = old.decoded();
    let fees = SimulationFeeFacts {
        gas_limit: decoded
            .gas_limit()
            .parse()
            .map_err(|_| QualificationError::InvalidIntent)?,
        max_fee_per_gas: decoded
            .max_fee_per_gas_wei()
            .parse()
            .map_err(|_| QualificationError::InvalidIntent)?,
        priority_fee_per_gas: decoded
            .max_priority_fee_per_gas_wei()
            .parse()
            .map_err(|_| QualificationError::InvalidIntent)?,
    };
    let fresh = prepare_qualified_contract(
        workers,
        endpoint,
        old.intent(),
        &ContractBlock::Latest,
        fees,
        deployment,
        cancelled,
    )?;
    let allowed = U256::from_str_radix(old.maximum_fee_wei(), 10)
        .map_err(|_| QualificationError::Arithmetic)?;
    let required = U256::from_str_radix(fresh.maximum_fee_wei(), 10)
        .map_err(|_| QualificationError::Arithmetic)?;
    if old.payload() != fresh.payload()
        || old.semantic_commitment() != fresh.semantic_commitment()
        || old.identity().profile_revision() != fresh.identity().profile_revision()
        || old.identity().runtime_hash() != fresh.identity().runtime_hash()
        || old.identity().deployment_transaction_hash()
            != fresh.identity().deployment_transaction_hash()
        || old.action() != fresh.action()
        || old.balances() != fresh.balances()
        || old.allowance() != fresh.allowance()
        || required > allowed
    {
        return Err(QualificationError::SnapshotMismatch);
    }
    require_current(refreshed_at, &SimulationControl::start(cancelled))?;
    Ok(RefreshedQualifiedCandidate {
        refreshed_at,
        approved_digest: old.candidate_digest().into(),
        fresh,
    })
}
fn is_fresh(refreshed_at: Instant) -> bool {
    refreshed_at.elapsed() < MAX_REFRESH_AGE
}
fn require_current(
    refreshed_at: Instant,
    control: &SimulationControl<'_>,
) -> Result<(), QualificationError> {
    control.remaining()?;
    if !is_fresh(refreshed_at) {
        return Err(QualificationError::SnapshotMismatch);
    }
    Ok(())
}
fn validate_signed_body(
    payload: &[u8],
    account: &str,
    bytes: &[u8],
) -> Result<String, QualificationError> {
    let reject = QualificationError::InvalidIntent;
    if bytes.len() > 8512 {
        return Err(reject);
    }
    let mut remaining = bytes;
    let signed = Signed::<TxEip1559>::eip2718_decode(&mut remaining).map_err(|_| reject)?;
    let mut canonical = Vec::new();
    signed.eip2718_encode(&mut canonical);
    if !remaining.is_empty()
        || canonical != bytes
        || signed.signature().normalize_s().is_some()
        || signed.tx().encoded_for_signing() != payload
        || signed.recover_signer().map_err(|_| reject)?
            != Address::from_str(account).map_err(|_| reject)?
    {
        return Err(reject);
    }
    Ok(format!("{:#x}", keccak256(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{B256, Signature, TxKind, hex};
    use std::sync::atomic::Ordering;

    #[test]
    fn every_signed_child_rejection_and_postrun_expiry_has_one_public_result() {
        use evm_simulation::SimulationError;
        for forged in [
            SimulationError::InvalidInput,
            SimulationError::UnsupportedRules,
            SimulationError::Limit,
            SimulationError::Cancelled,
            SimulationError::StateUnavailable,
            SimulationError::InconsistentState,
            SimulationError::WorkerUnavailable,
            SimulationError::BaseEnvelopeRequired,
            SimulationError::TransactionRejected,
            SimulationError::UnsupportedExecution,
            SimulationError::Protocol,
        ] {
            let result = validate_base_signed_phase::<()>(
                || Err(QualificationError::Simulation(forged)),
                |()| true,
                || Ok(()),
            );
            assert_eq!(result, Err(QualificationError::IncompleteSimulation));
        }
        for postrun in [
            QualificationError::SnapshotMismatch,
            QualificationError::Simulation(SimulationError::Limit),
            QualificationError::Simulation(SimulationError::Cancelled),
        ] {
            assert_eq!(
                validate_base_signed_phase(|| Ok(()), |()| true, || Err(postrun)),
                Err(QualificationError::IncompleteSimulation)
            );
        }
        assert_eq!(
            validate_base_signed_phase(
                || Ok("forged child hash"),
                |hash| *hash == "host-known exact hash",
                || Ok(())
            ),
            Err(QualificationError::IncompleteSimulation)
        );
        assert_eq!(
            validate_base_signed_phase(
                || Ok("host-known exact hash"),
                |hash| *hash == "host-known exact hash",
                || Ok(())
            ),
            Ok(())
        );
    }

    #[test]
    fn refreshed_and_validated_evidence_expire_without_resetting_age()
    -> Result<(), QualificationError> {
        let cancelled = AtomicBool::new(false);
        let now = Instant::now();
        require_current(now, &SimulationControl::start(&cancelled))?;
        let old = now
            .checked_sub(MAX_REFRESH_AGE)
            .ok_or(QualificationError::SnapshotMismatch)?;
        assert_eq!(
            require_current(old, &SimulationControl::start(&cancelled)),
            Err(QualificationError::SnapshotMismatch)
        );
        let certificate = ValidatedContractExecution {
            refreshed_at: old,
            transaction_hash: String::new(),
            approved_digest: String::new(),
            fresh_digest: String::new(),
        };
        assert!(!certificate.is_fresh());
        cancelled.store(true, Ordering::Release);
        assert_eq!(
            require_current(now, &SimulationControl::start(&cancelled)),
            Err(QualificationError::Simulation(
                evm_simulation::SimulationError::Cancelled
            ))
        );
        Ok(())
    }

    #[test]
    fn signed_release_requires_exact_canonical_body_signer_and_low_s()
    -> Result<(), Box<dyn std::error::Error>> {
        let transaction = TxEip1559 {
            chain_id: 8453,
            nonce: 3,
            gas_limit: 100_000,
            max_fee_per_gas: 2,
            max_priority_fee_per_gas: 1,
            to: TxKind::Call(Address::repeat_byte(0x22)),
            ..Default::default()
        };
        let payload = transaction.encoded_for_signing();
        let r = B256::from(hex!(
            "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
        ));
        let signature =
            Signature::from_scalars_and_parity(r, B256::from(U256::ONE.to_be_bytes::<32>()), false);
        let signed = Signed::new_unhashed(transaction.clone(), signature);
        let account = format!("{:#x}", signed.recover_signer()?);
        let mut bytes = Vec::new();
        signed.eip2718_encode(&mut bytes);
        assert_eq!(
            validate_signed_body(&payload, &account, &bytes)?,
            format!("{:#x}", keccak256(&bytes))
        );
        assert!(validate_signed_body(&payload, &format!("{:#x}", Address::ZERO), &bytes).is_err());
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(validate_signed_body(&payload, &account, &trailing).is_err());
        let mut changed = transaction.clone();
        changed.nonce += 1;
        assert!(validate_signed_body(&changed.encoded_for_signing(), &account, &bytes).is_err());
        let high_s = B256::from(hex!(
            "fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364140"
        ));
        let high = Signed::new_unhashed(
            transaction,
            Signature::from_scalars_and_parity(r, high_s, true),
        );
        assert_eq!(high.recover_signer()?, signed.recover_signer()?);
        let mut high_bytes = Vec::new();
        high.eip2718_encode(&mut high_bytes);
        assert!(validate_signed_body(&payload, &account, &high_bytes).is_err());
        assert!(validate_signed_body(&payload, &account, &vec![0; 8513]).is_err());
        Ok(())
    }
}
