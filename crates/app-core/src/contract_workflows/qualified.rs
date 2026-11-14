//! Only sealed chain certificates can prepare or credit a scoped workflow.
use super::{ContractWorkflowScope, execution::ContractWorkflowStepWork};
use crate::{AppCore, AppCoreError, EvmNonceReservation};
use app_storage::{
    ContractStepClaim, ContractWorkflowId, StorageError, StoredContractObserver,
    StoredObserverPredicate, StoredObserverResult, StoredPredicateComparison,
    StoredWorkflowCandidate, StoredWorkflowObservation,
};
use chain_evm::{
    contract::{
        ContractPredicate, PredicateComparison, QualifiedContractCandidate,
        VerifiedReceiptPredicates,
    },
    eip1559_signing_digest,
};
use sha2::{Digest, Sha256};
use std::{fmt::Write as _, sync::Arc};

/// Exact prepared workflow evidence, never native approval/authentication.
/// The enclosed candidate can only originate in the chain owner's qualifier.
pub struct PreparedContractWorkflowStep {
    work: ContractWorkflowStepWork,
    candidate: Arc<QualifiedContractCandidate>,
    context: StoredWorkflowCandidate,
    commitment: String,
}
impl ContractWorkflowStepWork {
    /// Binds an opaque independently qualified candidate to the fixed template.
    /// Goal observers are derived from qualified effects, not supplied by a model.
    ///
    /// # Errors
    /// Rejects different semantics/profiles, unknown coverage or stale/rebound state.
    pub fn bind_candidate(
        self,
        candidate: QualifiedContractCandidate,
    ) -> Result<PreparedContractWorkflowStep, AppCoreError> {
        let result = prepare_context(
            &self.scope,
            &self.intent,
            &candidate,
            self.claim.intent_commitment(),
        );
        let context = match result {
            Ok(context) => context,
            Err(error) => {
                let _ = self.stop_before_send(app_storage::ContractStepState::Failed);
                return Err(error.into());
            }
        };
        let commitment = context.commitment()?;
        if !self.bind_candidate_context(&context)? {
            return Err(
                StorageError::InvalidRecord("workflow candidate already bound or retired").into(),
            );
        }
        Ok(PreparedContractWorkflowStep {
            work: self,
            candidate: Arc::new(candidate),
            context,
            commitment,
        })
    }
}
impl PreparedContractWorkflowStep {
    /// Records denial, cancellation or failed preparation before transport admission.
    ///
    /// # Errors
    /// Rejects nonterminal or stale transitions.
    pub fn stop_before_send(
        &self,
        state: app_storage::ContractStepState,
    ) -> Result<bool, AppCoreError> {
        self.work.stop_before_send(state)
    }
    /// Exact ordered step index in the immutable semantic workflow.
    #[must_use]
    pub const fn index(&self) -> u8 {
        self.work.index()
    }
    /// Immutable scope shown by the native controller for every write.
    #[must_use]
    pub const fn scope(&self) -> &ContractWorkflowScope {
        &self.work.scope
    }
    /// Immutable opaque candidate; sharing it creates no signing permission.
    #[must_use]
    pub fn candidate(&self) -> &QualifiedContractCandidate {
        &self.candidate
    }
    /// Shared immutable candidate for the existing native approval registry.
    #[must_use]
    pub fn shared_candidate(&self) -> Arc<QualifiedContractCandidate> {
        Arc::clone(&self.candidate)
    }
    /// Exact structural prepared context; never a verified-result constructor.
    #[must_use]
    pub const fn context(&self) -> &StoredWorkflowCandidate {
        &self.context
    }
    /// Exact prepared context commitment retained by storage.
    #[must_use]
    pub fn commitment(&self) -> &str {
        &self.commitment
    }
    /// Host-derived current-state predicates to prove after receipt inclusion.
    ///
    /// # Errors
    /// Rejects unsupported predicate families rather than treating raw words as goals.
    pub fn predicates(&self) -> Result<Vec<ContractPredicate>, AppCoreError> {
        self.context
            .observers
            .iter()
            .map(predicate)
            .collect::<Result<_, _>>()
            .map_err(Into::into)
    }
    /// Credits only a newly minted opaque receipt/state certificate for this step.
    /// Persisted digests/observations cannot call this method.
    ///
    /// # Errors
    /// Rejects stale or mismatched scope, transaction, profile, predicates or proof.
    pub fn apply_verified_postcondition(
        &self,
        proof: VerifiedReceiptPredicates,
    ) -> Result<bool, AppCoreError> {
        let result = apply_certificate(&self.work.core, &self.work.claim, &self.work.scope, &proof);
        drop(proof); // Single-use capability: callers cannot retain a credited certificate.
        result
    }
    /// Consumes fresh prerequisite certificates before releasing a signature,
    /// then rechecks the prepared workflow, current turn and predecessor states.
    ///
    /// # Errors
    /// Rejects stale or invalidated authority; dispatch repeats the final fence.
    pub fn revalidate_preparation(
        &self,
        proofs: Vec<VerifiedReceiptPredicates>,
    ) -> Result<(), AppCoreError> {
        apply_prerequisites(
            &self.work.core,
            self.work.claim.workflow(),
            &self.work.scope,
            self.work.index(),
            proofs,
        )?;
        if !self.work.core.with_storage(|storage| {
            storage.contract_step_preparation_is_current(&self.work.claim, &self.commitment)
        })? {
            return Err(
                StorageError::InvalidRecord("prepared workflow authority was retired").into(),
            );
        }
        Ok(())
    }
    /// Atomically fences the workflow and exact nonce after the native controller
    /// re-proves every prerequisite following fresh password authentication.
    /// The claim-time certificates were consumed and cannot be reused here.
    ///
    /// # Errors
    /// Rejects missing/stale/newly failed prerequisite proof, scope or nonce state.
    pub fn mark_dispatched(
        &self,
        hash: &str,
        nonce: &EvmNonceReservation,
        prerequisites: Vec<VerifiedReceiptPredicates>,
    ) -> Result<bool, AppCoreError> {
        apply_prerequisites(
            &self.work.core,
            self.work.claim.workflow(),
            &self.work.scope,
            self.work.index(),
            prerequisites,
        )?;
        self.work.mark_dispatched(&self.commitment, hash, nonce)
    }
    /// Known-unsent cleanup, only in the actual callback-false path.
    ///
    /// # Errors
    /// Never use timeout/panic as no-send evidence. Storage faults retain blocking.
    pub fn discard_known_unsent(
        &self,
        hash: &str,
        nonce: &EvmNonceReservation,
    ) -> Result<bool, AppCoreError> {
        self.work.discard_known_unsent(hash, nonce)
    }
    /// Records post-admission execution facts, without crediting task success.
    ///
    /// # Errors
    /// Rejects impossible/stale state transitions or changed transaction identity.
    pub fn record_execution(
        &self,
        from: app_storage::ContractStepState,
        to: app_storage::ContractStepState,
        hash: Option<&str>,
    ) -> Result<bool, AppCoreError> {
        self.work
            .core
            .with_storage(|storage| storage.record_contract_step(&self.work.claim, from, to, hash))
    }
}

fn prepare_context(
    scope: &ContractWorkflowScope,
    intent: &app_storage::StoredWorkflowIntent,
    candidate: &QualifiedContractCandidate,
    intent_commitment: &str,
) -> Result<StoredWorkflowCandidate, StorageError> {
    let actual = candidate.intent();
    let identity = candidate.identity();
    if scope.intent().network != actual.network().descriptor().caip2_id()
        || scope.intent().account != actual.account()
        || intent.target != actual.call().target()
        || intent.operation != actual.call().signature()
        || intent.arguments != actual.call().arguments()
        || intent.value_wei != actual.value_wei()
        || intent.profile_id != identity.profile_id()
        || intent.profile_revision != 1
        || intent.profile_commitment != identity.profile_revision()
    {
        return Err(StorageError::InvalidRecord(
            "qualified candidate differs from immutable workflow intent",
        ));
    }
    let profile = |predicate| StoredContractObserver {
        profile_id: identity.profile_id().into(),
        profile_revision: 1,
        profile_commitment: identity.profile_revision().into(),
        predicate,
    };
    let mut observers = Vec::new();
    if let Some(effect) = candidate.allowance() {
        observers.push(profile(StoredObserverPredicate::Allowance {
            token: identity.target().into(),
            owner: effect.owner.clone(),
            spender: effect.spender.clone(),
            amount: effect.after.clone(),
        }));
    }
    for effect in candidate.balances() {
        observers.push(profile(StoredObserverPredicate::TokenBalance {
            token: identity.target().into(),
            account: effect.account.clone(),
            comparison: StoredPredicateComparison::Equal,
            amount: effect.after.clone(),
        }));
    }
    let source = serde_json::json!({"profile":identity.profile_id(),"revision":identity.profile_revision(),"runtime":identity.runtime_hash(),"creation":identity.deployment_transaction_hash(),"creationBlock":identity.deployment_block_hash(),"block":identity.block_hash()});
    let decoded = candidate.decoded();
    let financial = serde_json::json!({"nonce":decoded.nonce(),"gas":decoded.gas_limit(),"fee":decoded.max_fee_per_gas_wei(),"priority":decoded.max_priority_fee_per_gas_wei(),"maximum":candidate.maximum_fee_wei()});
    Ok(StoredWorkflowCandidate {
        intent_commitment: intent_commitment.into(),
        candidate_digest: candidate.candidate_digest().into(),
        signing_digest: hex(&eip1559_signing_digest(candidate.payload())),
        source_commitment: digest(&source),
        effects_commitment: candidate.effect_commitment().into(),
        financial_commitment: digest(&financial),
        nonce: decoded.nonce().into(),
        block_number: candidate.block_number().to_string(),
        block_hash: identity.block_hash().into(),
        observers,
    })
}

pub(super) fn apply_prerequisites(
    core: &AppCore,
    id: &ContractWorkflowId,
    scope: &ContractWorkflowScope,
    count: u8,
    proofs: Vec<VerifiedReceiptPredicates>,
) -> Result<(), AppCoreError> {
    if proofs.len() != usize::from(count) {
        return Err(
            StorageError::InvalidRecord("fresh proof required for every prerequisite").into(),
        );
    }
    let facts = proofs.iter().map(receipt_facts).collect::<Vec<_>>();
    validate_receipt_cohort(&facts)?;
    // Validate the entire cohort and every exact binding before the first
    // trusted refresh persistence call. Individual freshness is not a cohort.
    let updates = core.with_storage(|storage| {
        let steps = storage.contract_workflow_steps(id)?;
        validate_stored_prerequisites(&steps, &facts)?;
        facts
            .into_iter()
            .enumerate()
            .map(|(index, facts)| {
                let index = u8::try_from(index).map_err(|_| StorageError::CorruptDatabase)?;
                let step = steps
                    .get(usize::from(index))
                    .ok_or(StorageError::CorruptDatabase)?;
                let binding = storage
                    .contract_step_candidate(id, index)?
                    .ok_or(StorageError::CorruptDatabase)?;
                Ok((
                    index,
                    step.commitment.clone(),
                    from_facts(scope, index, &binding, facts)?,
                ))
            })
            .collect::<Result<Vec<_>, StorageError>>()
    })?;
    for (proof, (index, commitment, observation)) in proofs.into_iter().zip(updates) {
        if !proof.is_fresh() {
            return Err(StorageError::InvalidRecord(
                "prerequisite proof expired before persistence",
            )
            .into());
        }
        core.with_storage(|storage| {
            storage.refresh_verified_contract_step(id, index, &commitment, &observation)
        })?;
        drop(proof);
    }
    Ok(())
}
fn apply_certificate(
    core: &AppCore,
    claim: &ContractStepClaim,
    scope: &ContractWorkflowScope,
    proof: &VerifiedReceiptPredicates,
) -> Result<bool, AppCoreError> {
    core.with_storage(|storage| {
        let binding = storage
            .contract_step_candidate(claim.workflow(), claim.index())?
            .ok_or(StorageError::InvalidRecord(
                "prepared step context required",
            ))?;
        let observation = from_certificate(scope, claim.index(), &binding, proof)?;
        storage.apply_verified_contract_step_observation(claim, &observation)
    })
}
// Private immutable projection used only to compare certificate facts. It is
// neither serialisable nor a certificate and cannot credit a journal outcome.
struct ReceiptFacts {
    fresh: bool,
    candidate_digest: String,
    network: chain_evm::EvmNetwork,
    account: String,
    target: String,
    profile_id: String,
    profile_commitment: String,
    transaction_hash: String,
    receipt_block_number: u64,
    receipt_block_hash: String,
    state_block_number: u64,
    state_block_hash: String,
    receipt_commitment: String,
    proof_commitment: String,
    observations: Vec<(ContractPredicate, String)>,
}
fn from_certificate(
    scope: &ContractWorkflowScope,
    index: u8,
    binding: &StoredWorkflowCandidate,
    proof: &VerifiedReceiptPredicates,
) -> Result<StoredWorkflowObservation, StorageError> {
    let facts = receipt_facts(proof);
    from_facts(scope, index, binding, facts)
}
fn receipt_facts(proof: &VerifiedReceiptPredicates) -> ReceiptFacts {
    ReceiptFacts {
        fresh: proof.is_fresh(),
        candidate_digest: proof.candidate_digest().into(),
        network: proof.network(),
        account: proof.account().into(),
        target: proof.target().into(),
        profile_id: proof.profile_id().into(),
        profile_commitment: proof.profile_commitment().into(),
        transaction_hash: proof.transaction_hash().into(),
        receipt_block_number: proof.receipt_block_number(),
        receipt_block_hash: proof.receipt_block_hash().into(),
        state_block_number: proof.state_block_number(),
        state_block_hash: proof.state_block_hash().into(),
        receipt_commitment: proof.receipt_commitment().into(),
        proof_commitment: proof.proof_commitment().into(),
        observations: proof
            .observations()
            .iter()
            .map(|observation| (observation.predicate().clone(), observation.value().into()))
            .collect(),
    }
}
fn validate_stored_prerequisites(
    steps: &[app_storage::ContractWorkflowStep],
    facts: &[ReceiptFacts],
) -> Result<(), StorageError> {
    for (index, fact) in facts.iter().enumerate() {
        let step = steps.get(index).ok_or(StorageError::CorruptDatabase)?;
        if step.state != app_storage::ContractStepState::Verified
            || step.transaction_hash.as_deref() != Some(fact.transaction_hash.as_str())
        {
            return Err(StorageError::InvalidRecord(
                "prerequisite state or transaction hash differs from its stored step",
            ));
        }
    }
    Ok(())
}
fn validate_receipt_cohort(facts: &[ReceiptFacts]) -> Result<(), StorageError> {
    if let Some(first) = facts.first()
        && facts.iter().any(|fact| {
            !fact.fresh
                || fact.network != first.network
                || fact.account != first.account
                || fact.state_block_number != first.state_block_number
                || fact.state_block_hash != first.state_block_hash
        })
    {
        return Err(StorageError::InvalidRecord(
            "prerequisite proofs require one fresh canonical state block cohort",
        ));
    }
    Ok(())
}
fn from_facts(
    scope: &ContractWorkflowScope,
    index: u8,
    binding: &StoredWorkflowCandidate,
    facts: ReceiptFacts,
) -> Result<StoredWorkflowObservation, StorageError> {
    let intent = scope
        .intent()
        .steps
        .get(usize::from(index))
        .ok_or(StorageError::CorruptDatabase)?;
    if !facts.fresh
        || facts.candidate_digest != binding.candidate_digest
        || facts.network.descriptor().caip2_id() != scope.intent().network
        || facts.account != scope.intent().account
        || facts.target != intent.target
        || facts.profile_id != intent.profile_id
        || intent.profile_revision != 1
        || facts.profile_commitment != intent.profile_commitment
        || facts.observations.len() != binding.observers.len()
    {
        return Err(StorageError::InvalidRecord(
            "receipt certificate differs from workflow binding or is stale",
        ));
    }
    let mut results = Vec::new();
    for (expected, observed) in binding.observers.iter().zip(&facts.observations) {
        if expected.profile_id != facts.profile_id
            || expected.profile_revision != 1
            || expected.profile_commitment != facts.profile_commitment
            || !observer_target_matches(expected, &facts.target)
            || predicate(expected)? != observed.0
        {
            return Err(StorageError::InvalidRecord(
                "receipt predicate differs from prepared goal",
            ));
        }
        results.push(StoredObserverResult {
            observer: expected.clone(),
            value: observed.1.as_str().into(),
        });
    }
    Ok(StoredWorkflowObservation {
        candidate_commitment: binding.commitment()?,
        transaction_hash: facts.transaction_hash.to_ascii_lowercase(),
        receipt_block_number: facts.receipt_block_number.to_string(),
        receipt_block_hash: facts.receipt_block_hash.to_ascii_lowercase(),
        state_block_number: facts.state_block_number.to_string(),
        state_block_hash: facts.state_block_hash.to_ascii_lowercase(),
        receipt_commitment: facts.receipt_commitment,
        proof_commitment: facts.proof_commitment,
        results,
    })
}
fn predicate(observer: &StoredContractObserver) -> Result<ContractPredicate, StorageError> {
    match &observer.predicate {
        StoredObserverPredicate::Allowance {
            token,
            owner,
            spender,
            amount,
        } if !token.is_empty() => Ok(ContractPredicate::Allowance {
            owner: owner.clone(),
            spender: spender.clone(),
            amount: amount.clone(),
        }),
        StoredObserverPredicate::TokenBalance {
            token,
            account,
            comparison,
            amount,
        } if !token.is_empty() => Ok(ContractPredicate::TokenBalance {
            account: account.clone(),
            comparison: match comparison {
                StoredPredicateComparison::Equal => PredicateComparison::Equal,
                StoredPredicateComparison::AtLeast => PredicateComparison::AtLeast,
                StoredPredicateComparison::AtMost => PredicateComparison::AtMost,
            },
            amount: amount.clone(),
        }),
        _ => Err(StorageError::InvalidRecord(
            "observer has no qualified predicate adapter",
        )),
    }
}
fn hex(bytes: &[u8]) -> String {
    let mut result = String::from("0x");
    for byte in bytes {
        let _ = write!(result, "{byte:02x}");
    }
    result
}
fn digest(value: &serde_json::Value) -> String {
    format!("{:x}", Sha256::digest(value.to_string().as_bytes()))
}

fn observer_target_matches(observer: &StoredContractObserver, target: &str) -> bool {
    match &observer.predicate {
        StoredObserverPredicate::Allowance { token, .. }
        | StoredObserverPredicate::TokenBalance { token, .. } => token == target,
        StoredObserverPredicate::ApplicationWord { .. } => false,
    }
}

#[cfg(test)]
#[path = "qualified_tests.rs"]
mod tests;
