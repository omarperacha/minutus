//! Only opaque generic chain certificates can bind or complete declared workflows.
use super::{ContractWorkflowDeclaration, DeclaredContractWorkflowStep};
use crate::{AppCoreError, EvmNonceReservation};
use app_storage::{
    ContractStepState, StorageError, StoredGenericWorkflowEvidence,
    StoredGenericWorkflowPreparation, StoredWorkflowComparison, StoredWorkflowCondition,
    StoredWorkflowConditionResult, StoredWorkflowEvidenceKind, StoredWorkflowRead,
    StoredWorkflowReceiptAnchor,
};
use chain_evm::{
    contract::generic::{
        GenericComparison, GenericCondition, GenericContractCandidate, VerifiedGenericCompletion,
        VerifiedGenericConditions, VerifiedGenericPrerequisites,
    },
    eip1559_signing_digest,
};
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// Exact immutable generic candidate with host-owned workflow binding.
pub struct PreparedDeclaredContractStep {
    work: DeclaredContractWorkflowStep,
    candidate: Arc<GenericContractCandidate>,
    context: StoredGenericWorkflowPreparation,
    commitment: String,
}
/// Historical completion capability retained only by the live host controller.
/// It proves no current values and creates no signing permission after restart.
pub struct DeclaredContractCompletion {
    index: u8,
    proof: VerifiedGenericCompletion,
}
impl DeclaredContractCompletion {
    /// Exact completed ordered step.
    #[must_use]
    pub const fn index(&self) -> u8 {
        self.index
    }
    /// Opaque historical anchor for fresh canonical revalidation.
    #[must_use]
    pub const fn proof(&self) -> &VerifiedGenericCompletion {
        &self.proof
    }
}
impl DeclaredContractWorkflowStep {
    /// Binds independently qualified complete generic effects and fresh conditions.
    ///
    /// # Errors
    /// Rejects changed semantics, missing historical anchors or stale proof context.
    pub fn bind_generic_candidate(
        self,
        candidate: GenericContractCandidate,
        prerequisites: VerifiedGenericPrerequisites,
    ) -> Result<PreparedDeclaredContractStep, AppCoreError> {
        let declaration = self.declaration.intent();
        let target = declaration
            .contracts
            .iter()
            .find(|contract| contract.id == self.intent.contract)
            .ok_or(StorageError::CorruptDatabase)?;
        let intent = candidate.intent();
        if intent.network().descriptor().caip2_id() != declaration.network
            || intent.account() != declaration.account
            || intent.call().target() != target.address
            || intent.call().signature() != self.intent.function
            || intent.call().arguments() != self.intent.arguments
            || intent.value_wei() != self.intent.value_wei
            || !candidate.is_fresh()
        {
            return Err(StorageError::InvalidRecord(
                "generic candidate differs from immutable workflow declaration",
            )
            .into());
        }
        validate_gate(&self, &candidate, &prerequisites, None)?;
        let financial = serde_json::json!({"nonce":candidate.decoded().nonce(),"gas":candidate.decoded().gas_limit(),"fee":candidate.decoded().max_fee_per_gas_wei(),"priority":candidate.decoded().max_priority_fee_per_gas_wei(),"maximum":candidate.maximum_fee_wei()});
        let context = StoredGenericWorkflowPreparation {
            scope_commitment: self.declaration.commitment().into(),
            intent_commitment: self.claim.intent_commitment().into(),
            candidate_digest: candidate.candidate_digest().into(),
            signing_digest: format!("0x{}", hex(&eip1559_signing_digest(candidate.payload()))),
            source_commitment: source_commitment(&candidate, prerequisites.conditions())?,
            source_identities: prerequisites.conditions().source_commitments().to_vec(),
            effects_commitment: candidate.effect_commitment().into(),
            financial_commitment: digest(&financial)?,
            nonce: candidate.decoded().nonce().into(),
            block_number: candidate.block_number().to_string(),
            block_hash: candidate.block_hash().into(),
        };
        let commitment = context.commitment()?;
        if !self.core.with_storage(|storage| {
            storage.bind_generic_contract_preparation(&self.claim, &context)
        })? {
            return Err(StorageError::InvalidRecord(
                "generic preparation already consumed or retired",
            )
            .into());
        }
        drop(prerequisites); // Binding consumes this independently checked cohort.
        Ok(PreparedDeclaredContractStep {
            work: self,
            candidate: Arc::new(candidate),
            context,
            commitment,
        })
    }
}
impl PreparedDeclaredContractStep {
    /// Exact qualified candidate, never an authentication/approval capability.
    #[must_use]
    pub fn candidate(&self) -> &GenericContractCandidate {
        &self.candidate
    }
    /// Immutable sharing for host refresh workers.
    #[must_use]
    pub fn shared_candidate(&self) -> Arc<GenericContractCandidate> {
        Arc::clone(&self.candidate)
    }
    /// Full immutable generic declaration.
    #[must_use]
    pub const fn declaration(&self) -> &ContractWorkflowDeclaration {
        &self.work.declaration
    }
    /// Ordered exact step index.
    #[must_use]
    pub const fn index(&self) -> u8 {
        self.work.index()
    }
    /// Exact persisted preparation context.
    #[must_use]
    pub const fn context(&self) -> &StoredGenericWorkflowPreparation {
        &self.context
    }
    /// Current source-field requirements; unknown predicate engines fail closed.
    ///
    /// # Errors
    /// Rejects unresolved targets or unsupported ABI-read predicates.
    pub fn prerequisites(&self) -> Result<Vec<GenericCondition>, StorageError> {
        conditions(&self.work.declaration, &self.work.intent.prerequisites)
    }
    /// Declared historical completion predicates, separate from future requirements.
    ///
    /// # Errors
    /// Rejects unresolved targets or unsupported observation engines.
    pub fn completion_conditions(&self) -> Result<Vec<GenericCondition>, StorageError> {
        conditions(&self.work.declaration, &self.work.intent.completion)
    }
    /// Fresh certificate consumption immediately before releasing a signature.
    ///
    /// # Errors
    /// Rejects stale bindings, current-turn changes or historical invalidation.
    pub fn revalidate_preparation(
        &self,
        proof: VerifiedGenericPrerequisites,
    ) -> Result<(), AppCoreError> {
        validate_gate(
            &self.work,
            &self.candidate,
            &proof,
            Some(&self.context.source_commitment),
        )?;
        if !self.work.core.with_storage(|storage| {
            storage.generic_contract_preparation_is_current(&self.work.claim, &self.commitment)
        })? {
            return Err(
                StorageError::InvalidRecord("declared workflow preparation was retired").into(),
            );
        }
        if !proof.conditions().is_fresh() {
            return Err(
                StorageError::InvalidRecord("generic prerequisite evidence expired").into(),
            );
        }
        drop(proof); // Signing must not retain a reusable prerequisite certificate.
        Ok(())
    }
    /// Joins the fresh certificate, immutable workflow and shared nonce fence.
    ///
    /// # Errors
    /// Rejects changed prerequisites, ownership or storage state.
    pub fn mark_dispatched(
        &self,
        hash: &str,
        nonce: &EvmNonceReservation,
        proof: VerifiedGenericPrerequisites,
    ) -> Result<bool, AppCoreError> {
        validate_gate(
            &self.work,
            &self.candidate,
            &proof,
            Some(&self.context.source_commitment),
        )?;
        let evidence = observation(
            &self.work.declaration,
            proof.conditions(),
            &self.work.intent.prerequisites,
            StoredWorkflowEvidenceKind::Prerequisites,
            Some(self.commitment.clone()),
            None,
        )?;
        if !proof.conditions().is_fresh() {
            return Err(StorageError::InvalidRecord(
                "generic prerequisite expired before dispatch",
            )
            .into());
        }
        let result = self.work.core.with_storage(|storage| {
            storage.mark_generic_contract_dispatched(
                &self.work.claim,
                &self.commitment,
                hash,
                nonce.claim(),
                &evidence,
            )
        });
        drop(proof); // Actual dispatch consumes its final fresh certificate.
        result
    }
    /// Credits historical completion and retains its opaque canonical anchors.
    ///
    /// # Errors
    /// Rejects stale/mismatched source, candidate, transaction or declared conditions.
    pub fn apply_completion(
        &self,
        proof: VerifiedGenericCompletion,
    ) -> Result<Option<DeclaredContractCompletion>, AppCoreError> {
        if proof.candidate_digest() != self.candidate.candidate_digest()
            || proof.account() != self.work.declaration.intent().account
            || source_commitment(&self.candidate, proof.conditions())?
                != self.context.source_commitment
        {
            return Err(StorageError::InvalidRecord(
                "generic completion differs from prepared candidate",
            )
            .into());
        }
        let receipt = StoredWorkflowReceiptAnchor {
            transaction_hash: proof.transaction_hash().into(),
            block_number: proof.receipt_block_number().to_string(),
            block_hash: proof.receipt_block_hash().into(),
            inclusion_commitment: proof.inclusion_commitment().into(),
        };
        let evidence = observation(
            &self.work.declaration,
            proof.conditions(),
            &self.work.intent.completion,
            StoredWorkflowEvidenceKind::Completion,
            Some(self.commitment.clone()),
            Some(receipt),
        )?;
        if !proof.conditions().is_fresh() {
            return Err(StorageError::InvalidRecord("generic completion evidence expired").into());
        }
        let applied = self.work.core.with_storage(|storage| {
            storage.apply_generic_contract_completion(&self.work.claim, &evidence)
        })?;
        Ok(applied.then_some(DeclaredContractCompletion {
            index: self.index(),
            proof,
        }))
    }
    /// Records execution facts without converting receipt success into completion.
    ///
    /// # Errors
    /// Rejects impossible transitions or changed transaction identity.
    pub fn record_execution(
        &self,
        from: ContractStepState,
        to: ContractStepState,
        hash: Option<&str>,
    ) -> Result<bool, AppCoreError> {
        self.work
            .core
            .with_storage(|storage| storage.record_contract_step(&self.work.claim, from, to, hash))
    }
    /// Records denial, cancellation or failure before a possible send.
    ///
    /// # Errors
    /// Rejects stale or illegal lifecycle transitions.
    pub fn stop_before_send(&self, state: ContractStepState) -> Result<bool, AppCoreError> {
        self.work.stop_before_send(state)
    }
    /// Retires only an exact owner whose actual transport callback returned false.
    ///
    /// # Errors
    /// Never use panic, timeout or missing acknowledgement as no-send evidence.
    pub fn discard_known_unsent(
        &self,
        hash: &str,
        nonce: &EvmNonceReservation,
    ) -> Result<bool, AppCoreError> {
        self.work.core.with_storage(|storage| {
            storage.discard_known_unsent_generic_contract(&self.work.claim, hash, nonce.claim())
        })
    }
}
fn validate_gate(
    work: &DeclaredContractWorkflowStep,
    candidate: &GenericContractCandidate,
    proof: &VerifiedGenericPrerequisites,
    source: Option<&str>,
) -> Result<(), AppCoreError> {
    if proof.candidate_digest() != candidate.candidate_digest()
        || !proof.conditions().is_fresh()
        || source.is_some_and(|expected| {
            source_commitment(candidate, proof.conditions())
                .ok()
                .as_deref()
                != Some(expected)
        })
    {
        return Err(StorageError::InvalidRecord(
            "fresh generic prerequisite certificate differs from candidate/source",
        )
        .into());
    }
    observation(
        &work.declaration,
        proof.conditions(),
        &work.intent.prerequisites,
        StoredWorkflowEvidenceKind::Prerequisites,
        None,
        None,
    )?;
    work.core.with_storage(|storage| {
        let steps = storage.contract_workflow_steps(work.claim.workflow())?;
        let previous = &steps[..usize::from(work.index())];
        for prior in previous {
            let prepared = storage
                .generic_contract_preparation(work.claim.workflow(), prior.index)?
                .ok_or(StorageError::CorruptDatabase)?;
            if prepared
                .source_identities
                .iter()
                .any(|source| !proof.conditions().source_commitments().contains(source))
            {
                return Err(StorageError::InvalidRecord(
                    "a previously qualified workflow source identity disappeared",
                ));
            }
        }
        if previous
            .iter()
            .any(|step| step.state != ContractStepState::Verified)
            || proof.historical_transactions().len() != previous.len()
            || previous
                .iter()
                .zip(proof.historical_transactions())
                .any(|(step, hash)| step.transaction_hash.as_deref() != Some(hash.as_str()))
        {
            return Err(StorageError::InvalidRecord(
                "generic prerequisites differ from exact historical workflow completions",
            ));
        }
        Ok(())
    })
}
pub(super) fn conditions(
    declaration: &ContractWorkflowDeclaration,
    declared: &[StoredWorkflowCondition],
) -> Result<Vec<GenericCondition>, StorageError> {
    declared
        .iter()
        .map(|condition| {
            let target = declaration
                .intent()
                .contracts
                .iter()
                .find(|contract| contract.id == condition.contract)
                .ok_or(StorageError::CorruptDatabase)?;
            let StoredWorkflowRead::SourceField { path } = &condition.read else {
                return Err(StorageError::InvalidRecord(
                    "ABI-read conditions do not yet have a qualified executor",
                ));
            };
            Ok(GenericCondition {
                target: target.address.clone(),
                path: path
                    .iter()
                    .map(|part| {
                        part.as_str()
                            .map(str::to_owned)
                            .ok_or(StorageError::InvalidRecord("invalid source field path"))
                    })
                    .collect::<Result<_, _>>()?,
                comparison: match condition.comparison {
                    StoredWorkflowComparison::Equal => GenericComparison::Equal,
                    StoredWorkflowComparison::AtLeast => GenericComparison::AtLeast,
                    StoredWorkflowComparison::AtMost => GenericComparison::AtMost,
                },
                expected: condition.expected.clone(),
            })
        })
        .collect()
}
pub(super) fn observation(
    declaration: &ContractWorkflowDeclaration,
    proof: &VerifiedGenericConditions,
    declared: &[StoredWorkflowCondition],
    kind: StoredWorkflowEvidenceKind,
    preparation_commitment: Option<String>,
    receipt: Option<StoredWorkflowReceiptAnchor>,
) -> Result<StoredGenericWorkflowEvidence, StorageError> {
    let expected = conditions(declaration, declared)?;
    if !proof.is_fresh()
        || proof.network().descriptor().caip2_id() != declaration.intent().network
        || expected.len() != proof.observations().len()
        || expected
            .iter()
            .zip(proof.observations())
            .any(|(expected, observed)| expected != observed.condition())
    {
        return Err(StorageError::InvalidRecord(
            "generic condition certificate differs from immutable declaration",
        ));
    }
    Ok(StoredGenericWorkflowEvidence {
        kind,
        scope_commitment: declaration.commitment().into(),
        preparation_commitment,
        receipt,
        state_block_number: proof.block_number().to_string(),
        state_block_hash: proof.block_hash().into(),
        proof_commitment: proof.proof_commitment().into(),
        results: declared
            .iter()
            .zip(proof.observations())
            .map(|(condition, observed)| StoredWorkflowConditionResult {
                condition: condition.clone(),
                value: observed.value().clone(),
            })
            .collect(),
    })
}
fn source_commitment(
    candidate: &GenericContractCandidate,
    proof: &VerifiedGenericConditions,
) -> Result<String, StorageError> {
    digest(
        &serde_json::json!({"candidateSource":candidate.identity().commitment(),"conditionSources":proof.source_commitments()}),
    )
}
fn digest(value: &serde_json::Value) -> Result<String, StorageError> {
    Ok(hex(&Sha256::digest(serde_json::to_vec(value).map_err(
        |_| StorageError::InvalidRecord("invalid generic evidence commitment"),
    )?)))
}
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(value, "{byte:02x}");
    }
    value
}
