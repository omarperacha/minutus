//! Fresh material equivalence preserves the original unsigned body and budget.
//! Signed Base data goes only to the sealed offline native handler; child errors
//! never acquire detailed diagnostics or online acquisition authority.
use super::{
    GenericContractCandidate, GenericError, GenericSourceProvider, envelope,
    preparation::prepare_generic_contract_in_scope,
};
use crate::{
    EvmRpcUrl,
    contract::ContractBlock,
    local_simulation::{self, SimulationFeeFacts},
};
use alloy_primitives::U256;
use evm_simulation::{SimulationControl, TrustedSimulationWorkers};
use std::{
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

/// Material-equivalent fresh candidate, retaining the original approved binding.
/// Validation consumes it; no deserialisation or approval constructor exists.
pub struct RefreshedGenericCandidate {
    fresh: GenericContractCandidate,
    approved_digest: String,
    observed_at: Instant,
}
impl RefreshedGenericCandidate {
    /// Current candidate whose body/material remain within the original review.
    #[must_use]
    pub const fn fresh(&self) -> &GenericContractCandidate {
        &self.fresh
    }
    /// Monotonic age of the original refreshed proof, never reset by validation.
    #[must_use]
    pub fn is_fresh(&self) -> bool {
        self.observed_at.elapsed() < Duration::from_secs(30)
    }
    /// Exact fresh canonical state identity for current prerequisite binding.
    #[must_use]
    pub fn block_hash(&self) -> &str {
        self.fresh.block_hash()
    }
    /// Exact fresh state height.
    #[must_use]
    pub fn block_number(&self) -> u64 {
        self.fresh.block_number()
    }
    /// Validates canonical low-s signature, recovered account and exact approved
    /// body. Base additionally consumes the frozen state in the genuine signed
    /// native handler and checks its effects/settlement independently.
    ///
    /// # Errors
    /// Rejects expired/cancelled evidence, body mismatch, failed native execution,
    /// changed material or any unqualified signed output. Base post-input errors
    /// collapse to one fixed rejection rather than exposing child-derived data.
    pub fn validate_signed(
        self,
        workers: &TrustedSimulationWorkers,
        signed: &[u8],
        cancelled: &AtomicBool,
    ) -> Result<ValidatedGenericExecution, GenericError> {
        let control = SimulationControl::start(cancelled);
        current(self.observed_at, &control)?;
        let hash =
            envelope::signed_hash(self.fresh.payload(), self.fresh.intent().account(), signed)?;
        let digest = self.fresh.candidate_digest().to_owned();
        let block_hash = self.fresh.block_hash().to_owned();
        let block_number = self.fresh.block_number();
        if self.fresh.intent().network().is_base() {
            let execution = self
                .fresh
                .into_replay()
                .validate_base_signed(workers.offline(), signed, &control)
                .map_err(|_| GenericError::ExecutionRejected)?;
            if execution.transaction_hash() != hash {
                return Err(GenericError::ExecutionRejected);
            }
            current(self.observed_at, &control).map_err(|_| GenericError::ExecutionRejected)?;
        } else {
            current(self.observed_at, &control)?;
        }
        Ok(ValidatedGenericExecution {
            hash,
            approved: self.approved_digest,
            fresh: digest,
            observed_at: self.observed_at,
            block_hash,
            block_number,
        })
    }
}
/// Host-known signed hash and approved/fresh bindings. No child output, signature
/// or raw transaction bytes are projected by this receipt.
pub struct ValidatedGenericExecution {
    hash: String,
    approved: String,
    fresh: String,
    observed_at: Instant,
    block_hash: String,
    block_number: u64,
}
impl ValidatedGenericExecution {
    /// Hash independently computed from exact host-validated bytes.
    #[must_use]
    pub fn transaction_hash(&self) -> &str {
        &self.hash
    }
    /// Exact native-approved original candidate.
    #[must_use]
    pub fn approved_candidate_digest(&self) -> &str {
        &self.approved
    }
    /// Exact material-equivalent fresh candidate.
    #[must_use]
    pub fn fresh_candidate_digest(&self) -> &str {
        &self.fresh
    }
    /// Monotonic process-local validity, with no validation-time age reset.
    #[must_use]
    pub fn is_fresh(&self) -> bool {
        self.observed_at.elapsed() < Duration::from_secs(30)
    }
    /// Fresh state identity used for final predicate proof binding.
    #[must_use]
    pub fn evidence_block_hash(&self) -> &str {
        &self.block_hash
    }
    /// Fresh state height.
    #[must_use]
    pub const fn evidence_block_number(&self) -> u64 {
        self.block_number
    }
}

/// Selects current canonical state internally and preserves nonce, gas, fees,
/// value, calldata, source and all reviewed application effects. Actual protocol
/// fee movement may vary only inside the original independently reviewed cap.
///
/// # Errors
/// Rejects changed body, source/control identity, application effects, native
/// transfers, increased budget, missing proof, reorg or cancellation/expiry.
pub fn refresh_generic_contract(
    workers: &TrustedSimulationWorkers,
    endpoint: &EvmRpcUrl,
    approved: &GenericContractCandidate,
    sources: &mut GenericSourceProvider,
    cancelled: &AtomicBool,
) -> Result<RefreshedGenericCandidate, GenericError> {
    let control = SimulationControl::start(cancelled);
    let header = local_simulation::latest_header(endpoint, approved.intent().network(), &control)
        .map_err(|_| GenericError::SnapshotMismatch)?;
    let transaction = &approved.replay().request().transaction;
    let fees = SimulationFeeFacts {
        gas_limit: transaction.gas_limit,
        max_fee_per_gas: transaction.max_fee_per_gas,
        priority_fee_per_gas: transaction.priority_fee_per_gas,
    };
    let fresh = prepare_generic_contract_in_scope(
        workers,
        endpoint,
        approved.intent(),
        &ContractBlock::Hash(format!("{:#x}", header.hash_slow())),
        fees,
        sources,
        cancelled,
        approved.business_scope(),
    )?;
    if approved.payload() != fresh.payload()
        || approved.envelope().commitment() != fresh.envelope().commitment()
        || approved.source_commitment() != fresh.source_commitment()
        || approved.effects().material_commitment() != fresh.effects().material_commitment()
        || number(fresh.maximum_fee_wei())? > number(approved.maximum_fee_wei())?
        || number(fresh.maximum_total_debit_wei())? > number(approved.maximum_total_debit_wei())?
    {
        return Err(GenericError::SnapshotMismatch);
    }
    let observed_at = fresh.replay().prepared_at();
    current(observed_at, &control)?;
    Ok(RefreshedGenericCandidate {
        fresh,
        approved_digest: approved.candidate_digest().into(),
        observed_at,
    })
}
fn number(value: &str) -> Result<U256, GenericError> {
    U256::from_str_radix(value, 10).map_err(|_| GenericError::SnapshotMismatch)
}
fn current(observed_at: Instant, control: &SimulationControl<'_>) -> Result<(), GenericError> {
    control.remaining().map_err(|_| GenericError::Expired)?;
    if observed_at.elapsed() >= Duration::from_secs(30) {
        return Err(GenericError::Expired);
    }
    Ok(())
}
