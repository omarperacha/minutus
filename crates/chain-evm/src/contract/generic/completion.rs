//! Successful inclusion and fresh source-field assertions have separate lifetimes.
//! These private-field certificates cannot be recreated from persisted JSON.
use super::{
    GenericComparison, GenericCondition, GenericContractCandidate, GenericContractIdentity,
    GenericError, candidate_read::CandidateStateReader, layout,
};
use crate::{EvmNetwork, EvmRpcUrl, contract::DeploymentProof, local_simulation};

mod stages;
use alloy_primitives::{Address, I256, U256};
use evm_simulation::{SimulationControl, StateQuery};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
pub use stages::{GenericCompletionFailure, VerifiedGenericInclusion};
use std::{
    collections::BTreeMap,
    str::FromStr,
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

/// One independently proved source-field value. Names retain source provenance,
/// without claiming token, permission or financial semantics.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenericConditionObservation {
    condition: GenericCondition,
    solidity_type: String,
    value: Value,
}
impl GenericConditionObservation {
    /// Exact condition whose canonical typed value was proved.
    #[must_use]
    pub const fn condition(&self) -> &GenericCondition {
        &self.condition
    }
    /// Exact compiler-layout type.
    #[must_use]
    pub fn solidity_type(&self) -> &str {
        &self.solidity_type
    }
    /// Canonical actual value.
    #[must_use]
    pub const fn value(&self) -> &Value {
        &self.value
    }
}

/// Fresh current requirements, not historical transaction completion.
pub struct VerifiedGenericConditions {
    observed_at: Instant,
    network: EvmNetwork,
    source_commitments: Vec<String>,
    block_number: u64,
    block_hash: String,
    proof_commitment: String,
    observations: Vec<GenericConditionObservation>,
}
impl VerifiedGenericConditions {
    /// Monotonic process-local freshness, never reconstructible from persistence.
    #[must_use]
    pub fn is_fresh(&self) -> bool {
        self.observed_at.elapsed() < Duration::from_secs(30)
    }
    /// Exact network selected by the native controller.
    #[must_use]
    pub const fn network(&self) -> EvmNetwork {
        self.network
    }
    /// Complete ordered source identities revalidated at this snapshot.
    #[must_use]
    pub fn source_commitments(&self) -> &[String] {
        &self.source_commitments
    }
    /// Canonical observation state height.
    #[must_use]
    pub const fn block_number(&self) -> u64 {
        self.block_number
    }
    /// Canonical common observation state identity.
    #[must_use]
    pub fn block_hash(&self) -> &str {
        &self.block_hash
    }
    /// Source/state/condition evidence commitment.
    #[must_use]
    pub fn proof_commitment(&self) -> &str {
        &self.proof_commitment
    }
    /// Complete ordered independently checked values.
    #[must_use]
    pub fn observations(&self) -> &[GenericConditionObservation] {
        &self.observations
    }
}

/// Exact successful transaction inclusion plus historical completion observations.
/// Retaining this certificate does not keep its old values current; a later gate
/// rechecks its canonical anchors, then proves only newly declared requirements.
pub struct VerifiedGenericCompletion {
    candidate_digest: String,
    account: String,
    transaction_hash: String,
    receipt_number: u64,
    receipt_hash: String,
    inclusion_commitment: String,
    conditions: VerifiedGenericConditions,
}
impl VerifiedGenericCompletion {
    /// Exact included candidate.
    #[must_use]
    pub fn candidate_digest(&self) -> &str {
        &self.candidate_digest
    }
    /// Exact independently recovered signing account.
    #[must_use]
    pub fn account(&self) -> &str {
        &self.account
    }
    /// Exact canonical signed transaction identity.
    #[must_use]
    pub fn transaction_hash(&self) -> &str {
        &self.transaction_hash
    }
    /// Successful receipt height.
    #[must_use]
    pub const fn receipt_block_number(&self) -> u64 {
        self.receipt_number
    }
    /// Successful receipt identity.
    #[must_use]
    pub fn receipt_block_hash(&self) -> &str {
        &self.receipt_hash
    }
    /// Exact transaction/receipt trie-proof commitment.
    #[must_use]
    pub fn inclusion_commitment(&self) -> &str {
        &self.inclusion_commitment
    }
    /// Historical observation context; freshness is separately checked when credited.
    #[must_use]
    pub const fn conditions(&self) -> &VerifiedGenericConditions {
        &self.conditions
    }
}

/// Fresh current requirements plus canonically revalidated historical completions.
pub struct VerifiedGenericPrerequisites {
    candidate_digest: String,
    historical: Vec<String>,
    conditions: VerifiedGenericConditions,
}

/// Final current workflow goals, independently checked after all inclusions.
pub struct VerifiedGenericGoals {
    account: String,
    historical: Vec<String>,
    conditions: VerifiedGenericConditions,
}
impl VerifiedGenericGoals {
    /// Exact public workflow account retained by all historical completions.
    #[must_use]
    pub fn account(&self) -> &str {
        &self.account
    }
    /// Exact ordered included transactions whose anchors remain canonical.
    #[must_use]
    pub fn historical_transactions(&self) -> &[String] {
        &self.historical
    }
    /// Fresh current goals, independent of previous after-state observations.
    #[must_use]
    pub const fn conditions(&self) -> &VerifiedGenericConditions {
        &self.conditions
    }
}
impl VerifiedGenericPrerequisites {
    /// Exact next candidate for which this gate was checked.
    #[must_use]
    pub fn candidate_digest(&self) -> &str {
        &self.candidate_digest
    }
    /// Exact ordered previous transaction identities whose anchors remain canonical.
    #[must_use]
    pub fn historical_transactions(&self) -> &[String] {
        &self.historical
    }
    /// Fresh common-block requirements, with no requirement to retain old values.
    #[must_use]
    pub const fn conditions(&self) -> &VerifiedGenericConditions {
        &self.conditions
    }
}

/// Proves conditions using one internally selected latest canonical state cohort.
/// The caller must already possess native outbound consent. Unknown source/layout
/// targets and raw provider values cannot mint a certificate.
///
/// # Errors
/// Rejects source changes, missing/invalid proofs, unmet conditions and limits.
pub fn verify_generic_conditions(
    endpoint: &EvmRpcUrl,
    network: EvmNetwork,
    identities: &[&GenericContractIdentity],
    conditions: &[GenericCondition],
    cancelled: &AtomicBool,
) -> Result<VerifiedGenericConditions, GenericError> {
    let cohort = super::select_generic_live_cohort(endpoint, network, cancelled)?;
    verify_generic_conditions_at_cohort(endpoint, &cohort, identities, conditions, cancelled)
}

/// Proves current requirements at the exact host-selected live cohort without
/// selecting another latest block or resetting the selection clock.
///
/// # Errors
/// Rejects expired, foreign, reorganised or unsatisfied proof cohorts.
pub fn verify_generic_conditions_at_cohort(
    endpoint: &EvmRpcUrl,
    cohort: &super::GenericLiveCohort,
    identities: &[&GenericContractIdentity],
    conditions: &[GenericCondition],
    cancelled: &AtomicBool,
) -> Result<VerifiedGenericConditions, GenericError> {
    if identities.is_empty() || identities.len() > 32 || conditions.len() > 16 {
        return Err(GenericError::Limit);
    }
    if !cohort.is_fresh() {
        return Err(GenericError::Expired);
    }
    let control = SimulationControl::start(cancelled);
    let reader = CandidateStateReader::at_cohort(endpoint, cohort, &control)?;
    prove_conditions(cohort, identities, conditions, &control, reader)
}

/// Proves current requirements using only the candidate's sealed same-cohort
/// beforestate, with independent exact-hash proof acquisition for missing facts.
/// Existing zero values remain distinct from unavailable records.
///
/// # Errors
/// Rejects historical, stale or mismatched candidates, unavailable proofs,
/// changed source/control identities and unsatisfied current conditions.
pub fn verify_generic_candidate_conditions(
    endpoint: &EvmRpcUrl,
    candidate: &GenericContractCandidate,
    identities: &[&GenericContractIdentity],
    conditions: &[GenericCondition],
    cancelled: &AtomicBool,
) -> Result<VerifiedGenericConditions, GenericError> {
    let cohort = super::candidate_read::live_candidate_cohort(candidate)?;
    require_candidate_identity(candidate, identities)?;
    let control = SimulationControl::start(cancelled);
    let reader = CandidateStateReader::from_candidate(endpoint, candidate)?;
    prove_conditions(cohort, identities, conditions, &control, reader)
}

fn prove_conditions(
    cohort: &super::GenericLiveCohort,
    identities: &[&GenericContractIdentity],
    conditions: &[GenericCondition],
    control: &SimulationControl<'_>,
    mut reader: CandidateStateReader<'_>,
) -> Result<VerifiedGenericConditions, GenericError> {
    if identities.is_empty() || identities.len() > 32 || conditions.len() > 16 {
        return Err(GenericError::Limit);
    }
    if !cohort.is_fresh() {
        return Err(GenericError::Expired);
    }
    let network = cohort.network();
    let block_hash = cohort.block_hash().to_owned();
    let mut source_commitments = Vec::with_capacity(identities.len());
    for identity in identities {
        if identity.network() != network {
            return Err(GenericError::SnapshotMismatch);
        }
        let queries = identity.state_queries();
        if queries.is_empty() || queries.len() > 32 {
            return Err(GenericError::Limit);
        }
        for query in queries {
            reader.query(&query, control)?;
        }
        source_commitments.push(identity.commitment().into());
    }
    let mut reads = BTreeMap::new();
    let mut observations = Vec::with_capacity(conditions.len());
    let mut satisfied = true;
    for condition in conditions {
        let address = Address::from_str(&condition.target).map_err(|_| GenericError::InvalidAbi)?;
        if format!("{address:#x}") != condition.target {
            return Err(GenericError::InvalidAbi);
        }
        let artifact = condition_artifact(identities, address)?;
        let observed = layout::read_condition_path(
            artifact.storage_layout(),
            &condition.path,
            address,
            |query| {
                let StateQuery::Storage {
                    address: target,
                    slot,
                } = &query
                else {
                    return Err(GenericError::OpaqueEffects);
                };
                if target != &condition.target {
                    return Err(GenericError::SnapshotMismatch);
                }
                let slot = U256::from_str(slot).map_err(|_| GenericError::InvalidAbi)?;
                if let Some(value) = reads.get(&(address, slot)) {
                    return Ok(*value);
                }
                if reads.len() >= 128 {
                    return Err(GenericError::Limit);
                }
                let evm_simulation::StateValue::Storage { value } =
                    reader.query(&query, control)?
                else {
                    return Err(GenericError::MissingState);
                };
                let value = U256::from_str(&value).map_err(|_| GenericError::MissingState)?;
                reads.insert((address, slot), value);
                Ok(value)
            },
        )?;
        satisfied &= satisfies(condition, &observed.solidity_type, &observed.value)?;
        observations.push(GenericConditionObservation {
            condition: condition.clone(),
            solidity_type: observed.solidity_type,
            value: observed.value,
        });
    }
    let state = reader.finish_live(cohort, control)?;
    for identity in identities {
        identity.verify_state(&state)?;
    }
    if !cohort.is_fresh() {
        return Err(GenericError::Expired);
    }
    if !satisfied {
        return Err(GenericError::ConditionUnsatisfied);
    }
    let material = serde_json::json!({"domain":"minutus-generic-condition-proof-v1","network":network.descriptor().caip2_id(),"sources":source_commitments,"block":block_hash,"observations":observations});
    let proof_commitment = commitment(&material)?;
    Ok(VerifiedGenericConditions {
        observed_at: cohort.selected_at,
        network,
        source_commitments,
        block_number: cohort.block_number(),
        block_hash,
        proof_commitment,
        observations,
    })
}

/// Checks exact successful inclusion separately from fresh completion conditions.
///
/// # Errors
/// Rejects body/signer/hash/proof mismatch, reverts, reorgs and unmet conditions.
pub fn verify_generic_completion(
    endpoint: &EvmRpcUrl,
    candidate: &GenericContractCandidate,
    identities: &[&GenericContractIdentity],
    hash: &str,
    inclusion: &DeploymentProof,
    conditions: &[GenericCondition],
    cancelled: &AtomicBool,
) -> Result<VerifiedGenericCompletion, GenericError> {
    verify_generic_completion_staged(
        endpoint, candidate, identities, hash, inclusion, conditions, cancelled,
    )
    .map_err(GenericCompletionFailure::into_error)
}

/// Preserves the exact inclusion stage separately from later source/state checks.
/// A completion-stage error carries an opaque inclusion seal; raw acquisition
/// success, a receipt report or an error category cannot recreate that seal.
/// The original inclusion/final-recheck control remains in force. This split
/// adds no phase allowance; existing condition acquisition keeps its unchanged
/// independently bounded controls.
///
/// # Errors
/// Rejects the same inputs as [`verify_generic_completion`], identifying whether
/// exact canonical candidate inclusion had already been independently checked.
pub fn verify_generic_completion_staged(
    endpoint: &EvmRpcUrl,
    candidate: &GenericContractCandidate,
    identities: &[&GenericContractIdentity],
    hash: &str,
    inclusion: &DeploymentProof,
    conditions: &[GenericCondition],
    cancelled: &AtomicBool,
) -> Result<VerifiedGenericCompletion, GenericCompletionFailure> {
    let control = SimulationControl::start(cancelled);
    let network = candidate.intent().network();
    let header =
        local_simulation::canonical_header(endpoint, network, &inclusion.block_hash, &control)
            .map_err(|_| GenericCompletionFailure::Inclusion(GenericError::SnapshotMismatch))?;
    let checked = stages::verify(
        network,
        &header,
        candidate.payload(),
        candidate.intent().account(),
        hash,
        inclusion,
        candidate.block_number(),
    )
    .map_err(GenericCompletionFailure::Inclusion)?;
    stages::complete(checked, |checked| {
        finish_completion(
            endpoint, candidate, identities, checked, conditions, cancelled, &control,
        )
    })
}

fn finish_completion(
    endpoint: &EvmRpcUrl,
    candidate: &GenericContractCandidate,
    identities: &[&GenericContractIdentity],
    inclusion: &VerifiedGenericInclusion,
    conditions: &[GenericCondition],
    cancelled: &AtomicBool,
    control: &SimulationControl<'_>,
) -> Result<VerifiedGenericCompletion, GenericError> {
    let network = candidate.intent().network();
    require_candidate_identity(candidate, identities)?;
    let observed = verify_generic_conditions(endpoint, network, identities, conditions, cancelled)?;
    if observed.block_number < inclusion.receipt_number {
        return Err(GenericError::SnapshotMismatch);
    }
    local_simulation::canonical_header(endpoint, network, &inclusion.receipt_hash, control)
        .map_err(|_| GenericError::SnapshotMismatch)?;
    current_anchor(
        endpoint,
        network,
        observed.block_hash(),
        observed.block_number(),
        control,
    )?;
    if !observed.is_fresh() {
        return Err(GenericError::Expired);
    }
    Ok(VerifiedGenericCompletion {
        candidate_digest: candidate.candidate_digest().into(),
        account: candidate.intent().account().into(),
        transaction_hash: inclusion.transaction_hash.clone(),
        receipt_number: inclusion.receipt_number,
        receipt_hash: inclusion.receipt_hash.clone(),
        inclusion_commitment: inclusion.inclusion_commitment.clone(),
        conditions: observed,
    })
}

/// Rechecks historical anchors without asserting their previously observed values
/// still hold, then proves precisely this candidate's declared current conditions.
///
/// # Errors
/// Rejects foreign/reorganised anchors, source changes, stale state or limits.
pub fn verify_generic_prerequisites(
    endpoint: &EvmRpcUrl,
    candidate: &GenericContractCandidate,
    identities: &[&GenericContractIdentity],
    historical: &[&VerifiedGenericCompletion],
    conditions: &[GenericCondition],
    cancelled: &AtomicBool,
) -> Result<VerifiedGenericPrerequisites, GenericError> {
    require_candidate_identity(candidate, identities)?;
    let cohort = candidate
        .live_cohort
        .as_ref()
        .ok_or(GenericError::SnapshotMismatch)?;
    let observed =
        verify_generic_conditions_at_cohort(endpoint, cohort, identities, conditions, cancelled)?;
    bind_generic_prerequisites(endpoint, candidate, historical, observed, cancelled)
}

/// Binds a previously proved current cohort after a candidate was prepared at
/// that exact snapshot. The controller checks refreshed execution and this cohort
/// share a block before any signature; this method rechecks canonical anchors.
///
/// # Errors
/// Rejects stale/foreign conditions, missing candidate source or reorganised history.
pub fn bind_generic_prerequisites(
    endpoint: &EvmRpcUrl,
    candidate: &GenericContractCandidate,
    historical: &[&VerifiedGenericCompletion],
    observed: VerifiedGenericConditions,
    cancelled: &AtomicBool,
) -> Result<VerifiedGenericPrerequisites, GenericError> {
    let cohort = candidate
        .live_cohort
        .as_ref()
        .ok_or(GenericError::SnapshotMismatch)?;
    bind_at_cohort(
        endpoint,
        candidate,
        candidate.candidate_digest(),
        cohort,
        historical,
        observed,
        cancelled,
    )
}

/// Proves requirements at the refresh's sealed current cohort, retaining the
/// original approved body binding through the separate material-equivalence proof.
///
/// # Errors
/// Rejects stale, foreign, reorganised or unsatisfied current evidence.
pub fn verify_refreshed_generic_conditions(
    endpoint: &EvmRpcUrl,
    refreshed: &super::RefreshedGenericCandidate,
    identities: &[&GenericContractIdentity],
    conditions: &[GenericCondition],
    cancelled: &AtomicBool,
) -> Result<VerifiedGenericConditions, GenericError> {
    require_candidate_identity(refreshed.fresh(), identities)?;
    verify_generic_candidate_conditions(
        endpoint,
        refreshed.fresh(),
        identities,
        conditions,
        cancelled,
    )
}

/// Binds fresh conditions to a material-equivalent refreshed execution while
/// preserving the exact original approved candidate digest for the core gate.
///
/// # Errors
/// Rejects a different cohort, stale evidence, missing source or reorganised history.
pub fn bind_refreshed_generic_prerequisites(
    endpoint: &EvmRpcUrl,
    refreshed: &super::RefreshedGenericCandidate,
    historical: &[&VerifiedGenericCompletion],
    observed: VerifiedGenericConditions,
    cancelled: &AtomicBool,
) -> Result<VerifiedGenericPrerequisites, GenericError> {
    bind_at_cohort(
        endpoint,
        refreshed.fresh(),
        &refreshed.approved_digest,
        &refreshed.cohort,
        historical,
        observed,
        cancelled,
    )
}

fn bind_at_cohort(
    endpoint: &EvmRpcUrl,
    candidate: &GenericContractCandidate,
    approved_digest: &str,
    cohort: &super::GenericLiveCohort,
    historical: &[&VerifiedGenericCompletion],
    observed: VerifiedGenericConditions,
    cancelled: &AtomicBool,
) -> Result<VerifiedGenericPrerequisites, GenericError> {
    if historical.len() > 15 {
        return Err(GenericError::Limit);
    }
    let control = SimulationControl::start(cancelled);
    let network = candidate.intent().network();
    require_condition_cohort(cohort, &observed)?;
    if candidate.block_hash() != cohort.block_hash()
        || candidate.block_number() != cohort.block_number()
        || observed.network != network
        || cohort.network() != network
        || !observed
            .source_commitments
            .iter()
            .any(|source| source == candidate.identity().commitment())
    {
        return Err(GenericError::SnapshotMismatch);
    }
    let mut transactions = Vec::with_capacity(historical.len());
    for completion in historical {
        if completion.conditions.network != network
            || completion.account != candidate.intent().account()
        {
            return Err(GenericError::SnapshotMismatch);
        }
        let receipt = local_simulation::canonical_header(
            endpoint,
            network,
            &completion.receipt_hash,
            &control,
        )
        .map_err(|_| GenericError::SnapshotMismatch)?;
        let observation = local_simulation::canonical_header(
            endpoint,
            network,
            &completion.conditions.block_hash,
            &control,
        )
        .map_err(|_| GenericError::SnapshotMismatch)?;
        if receipt.number != completion.receipt_number
            || observation.number != completion.conditions.block_number
        {
            return Err(GenericError::SnapshotMismatch);
        }
        transactions.push(completion.transaction_hash.clone());
    }
    for completion in historical {
        if observed.block_number < completion.conditions.block_number {
            return Err(GenericError::SnapshotMismatch);
        }
        local_simulation::canonical_header(endpoint, network, &completion.receipt_hash, &control)
            .map_err(|_| GenericError::SnapshotMismatch)?;
        local_simulation::canonical_header(
            endpoint,
            network,
            &completion.conditions.block_hash,
            &control,
        )
        .map_err(|_| GenericError::SnapshotMismatch)?;
    }
    current_anchor(
        endpoint,
        network,
        observed.block_hash(),
        observed.block_number(),
        &control,
    )?;
    if !observed.is_fresh() {
        return Err(GenericError::Expired);
    }
    Ok(VerifiedGenericPrerequisites {
        candidate_digest: approved_digest.into(),
        historical: transactions,
        conditions: observed,
    })
}

/// Establishes final current goals while retaining every included historical step.
/// Old completion values may have been consumed; only their anchors are rechecked.
///
/// # Errors
/// Rejects absent/foreign/reorganised history, changed sources and unmet goals.
pub fn verify_generic_goals(
    endpoint: &EvmRpcUrl,
    network: EvmNetwork,
    identities: &[&GenericContractIdentity],
    historical: &[&VerifiedGenericCompletion],
    conditions: &[GenericCondition],
    cancelled: &AtomicBool,
) -> Result<VerifiedGenericGoals, GenericError> {
    if historical.is_empty() || historical.len() > 16 || conditions.is_empty() {
        return Err(GenericError::Limit);
    }
    let account = historical[0].account.clone();
    let control = SimulationControl::start(cancelled);
    let anchors = || -> Result<(), GenericError> {
        for completion in historical {
            if completion.conditions.network != network || completion.account != account {
                return Err(GenericError::SnapshotMismatch);
            }
            let receipt = local_simulation::canonical_header(
                endpoint,
                network,
                &completion.receipt_hash,
                &control,
            )
            .map_err(|_| GenericError::SnapshotMismatch)?;
            let observed = local_simulation::canonical_header(
                endpoint,
                network,
                &completion.conditions.block_hash,
                &control,
            )
            .map_err(|_| GenericError::SnapshotMismatch)?;
            if receipt.number != completion.receipt_number
                || observed.number != completion.conditions.block_number
            {
                return Err(GenericError::SnapshotMismatch);
            }
        }
        Ok(())
    };
    anchors()?;
    let observed = verify_generic_conditions(endpoint, network, identities, conditions, cancelled)?;
    if historical
        .iter()
        .any(|completion| completion.conditions.block_number > observed.block_number)
    {
        return Err(GenericError::SnapshotMismatch);
    }
    anchors()?;
    current_anchor(
        endpoint,
        network,
        observed.block_hash(),
        observed.block_number(),
        &control,
    )?;
    if !observed.is_fresh() {
        return Err(GenericError::Expired);
    }
    Ok(VerifiedGenericGoals {
        account,
        historical: historical
            .iter()
            .map(|completion| completion.transaction_hash.clone())
            .collect(),
        conditions: observed,
    })
}

fn require_candidate_identity(
    candidate: &GenericContractCandidate,
    identities: &[&GenericContractIdentity],
) -> Result<(), GenericError> {
    if identities
        .iter()
        .any(|identity| identity.commitment() == candidate.identity().commitment())
    {
        Ok(())
    } else {
        Err(GenericError::SourceMismatch)
    }
}
fn require_condition_cohort(
    cohort: &super::GenericLiveCohort,
    observed: &VerifiedGenericConditions,
) -> Result<(), GenericError> {
    if !observed.is_fresh() || !cohort.is_fresh() {
        return Err(GenericError::Expired);
    }
    if observed.observed_at != cohort.selected_at
        || observed.block_hash != cohort.block_hash()
        || observed.block_number != cohort.block_number()
        || observed.network != cohort.network()
    {
        return Err(GenericError::SnapshotMismatch);
    }
    Ok(())
}
fn current_anchor(
    endpoint: &EvmRpcUrl,
    network: EvmNetwork,
    expected: &str,
    number: u64,
    control: &SimulationControl<'_>,
) -> Result<(), GenericError> {
    super::cohort::canonical_live_anchor(endpoint, network, expected, number, control)
}
fn commitment(value: &impl Serialize) -> Result<String, GenericError> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).map_err(|_| GenericError::Limit)?)
    ))
}
pub(super) fn satisfies(
    condition: &GenericCondition,
    ty: &str,
    actual: &Value,
) -> Result<bool, GenericError> {
    if ty.starts_with("uint") {
        let bits = integer_bits(ty, "uint")?;
        let parse = |value: &Value| -> Result<U256, GenericError> {
            let text = value.as_str().ok_or(GenericError::InvalidAbi)?;
            let parsed = U256::from_str_radix(text, 10).map_err(|_| GenericError::InvalidAbi)?;
            if parsed.to_string() != text {
                return Err(GenericError::InvalidAbi);
            }
            if bits < 256 && parsed >> bits != U256::ZERO {
                return Err(GenericError::InvalidAbi);
            }
            Ok(parsed)
        };
        return Ok(compare(
            condition.comparison,
            parse(actual)?.cmp(&parse(&condition.expected)?),
        ));
    }
    if ty.starts_with("int") {
        let bits = integer_bits(ty, "int")?;
        let parse = |value: &Value| -> Result<I256, GenericError> {
            let text = value.as_str().ok_or(GenericError::InvalidAbi)?;
            let parsed = I256::from_str(text).map_err(|_| GenericError::InvalidAbi)?;
            if parsed.to_string() != text {
                return Err(GenericError::InvalidAbi);
            }
            if bits < 256 {
                let bound = I256::from_raw(U256::from(1) << (bits - 1));
                if parsed < -bound || parsed >= bound {
                    return Err(GenericError::InvalidAbi);
                }
            }
            Ok(parsed)
        };
        return Ok(compare(
            condition.comparison,
            parse(actual)?.cmp(&parse(&condition.expected)?),
        ));
    }
    if condition.comparison != GenericComparison::Equal {
        return Err(GenericError::InvalidAbi);
    }
    Ok(actual == &condition.expected)
}
pub(super) fn condition_artifact<'a>(
    identities: &[&'a GenericContractIdentity],
    address: Address,
) -> Result<&'a solidity_compilation::ReproducedArtifact, GenericError> {
    let mut artifacts = identities
        .iter()
        .filter_map(|identity| identity.storage_artifact(address));
    let artifact = artifacts.next().ok_or(GenericError::SourceMismatch)?;
    if artifacts.any(|other| other.commitment() != artifact.commitment()) {
        return Err(GenericError::SourceMismatch);
    }
    Ok(artifact)
}
fn integer_bits(ty: &str, prefix: &str) -> Result<usize, GenericError> {
    let bits = ty
        .strip_prefix(prefix)
        .and_then(|bits| bits.parse::<usize>().ok())
        .ok_or(GenericError::InvalidAbi)?;
    if !(8..=256).contains(&bits) || bits % 8 != 0 || ty != format!("{prefix}{bits}") {
        return Err(GenericError::InvalidAbi);
    }
    Ok(bits)
}
fn compare(comparison: GenericComparison, ordering: std::cmp::Ordering) -> bool {
    match comparison {
        GenericComparison::Equal => ordering.is_eq(),
        GenericComparison::AtLeast => !ordering.is_lt(),
        GenericComparison::AtMost => !ordering.is_gt(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn condition_binding_requires_original_live_selection_not_only_equal_height()
    -> Result<(), GenericError> {
        let cohort = super::super::GenericLiveCohort {
            network: EvmNetwork::EthereumSepolia,
            block_hash: "original".into(),
            block_number: 10,
            selected_at: Instant::now(),
        };
        let mut observed = VerifiedGenericConditions {
            network: cohort.network(),
            block_hash: cohort.block_hash().into(),
            block_number: 10,
            observed_at: cohort.selected_at,
            source_commitments: vec![],
            proof_commitment: String::new(),
            observations: vec![],
        };
        assert!(require_condition_cohort(&cohort, &observed).is_ok());
        observed.block_hash = "other".into();
        assert_eq!(
            require_condition_cohort(&cohort, &observed),
            Err(GenericError::SnapshotMismatch)
        );
        observed.block_hash = cohort.block_hash().into();
        observed.block_number = 11;
        assert_eq!(
            require_condition_cohort(&cohort, &observed),
            Err(GenericError::SnapshotMismatch)
        );
        observed.block_number = 10;
        observed.network = EvmNetwork::EthereumMainnet;
        assert_eq!(
            require_condition_cohort(&cohort, &observed),
            Err(GenericError::SnapshotMismatch)
        );
        observed.network = cohort.network();
        observed.observed_at = cohort
            .selected_at
            .checked_sub(Duration::from_millis(1))
            .ok_or(GenericError::Limit)?;
        assert_eq!(
            require_condition_cohort(&cohort, &observed),
            Err(GenericError::SnapshotMismatch)
        );
        observed.observed_at = Instant::now()
            .checked_sub(Duration::from_secs(31))
            .ok_or(GenericError::Limit)?;
        assert_eq!(
            require_condition_cohort(&cohort, &observed),
            Err(GenericError::Expired)
        );
        Ok(())
    }
    #[test]
    fn typed_comparison_rejects_coercion_and_preserves_signed_ordering() -> Result<(), GenericError>
    {
        let mut condition = GenericCondition {
            target: format!("0x{}", "1".repeat(40)),
            path: vec!["value".into()],
            comparison: GenericComparison::AtLeast,
            expected: json!("-2"),
        };
        assert!(satisfies(&condition, "int256", &json!("-1"))?);
        assert!(!satisfies(&condition, "int256", &json!("-3"))?);
        condition.expected = json!("2");
        assert!(satisfies(&condition, "uint256", &json!("10"))?);
        condition.expected = json!("02");
        assert!(satisfies(&condition, "uint256", &json!("10")).is_err());
        condition.expected = json!("256");
        assert!(satisfies(&condition, "uint8", &json!("10")).is_err());
        condition.expected = json!("128");
        assert!(satisfies(&condition, "int8", &json!("10")).is_err());
        condition.expected = json!(true);
        assert!(satisfies(&condition, "bool", &json!(true)).is_err());
        condition.comparison = GenericComparison::Equal;
        assert!(satisfies(&condition, "bool", &json!(true))?);
        assert!(!satisfies(&condition, "bool", &json!("true"))?);
        Ok(())
    }
}
