//! Structural generic execution records. Persistence never recreates chain authority.
use super::{declaration::StoredWorkflowCondition, records, scope};
use crate::StorageError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Immutable exact preparation context, independently bound by the application.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoredGenericWorkflowPreparation {
    /// Complete immutable workflow declaration commitment.
    pub scope_commitment: String,
    /// Exact immutable step commitment.
    pub intent_commitment: String,
    /// Exact native candidate, shared with nonce ownership.
    pub candidate_digest: String,
    /// Exact signing payload digest.
    pub signing_digest: String,
    /// Complete source/ABI/layout/dispatch graph commitment.
    pub source_commitment: String,
    /// Complete source/dispatch identities pinned across this workflow.
    pub source_identities: Vec<String>,
    /// Complete independently qualified execution effects commitment.
    pub effects_commitment: String,
    /// Exact financial terms shown in native approval.
    pub financial_commitment: String,
    /// Exact canonical account nonce.
    pub nonce: String,
    /// Canonical preparation state height.
    pub block_number: String,
    /// Canonical preparation state identity.
    pub block_hash: String,
}
impl StoredGenericWorkflowPreparation {
    /// Commits structural preparation context, without granting approval.
    ///
    /// # Errors
    /// Rejects malformed commitments, identities and resource excess.
    pub fn commitment(&self) -> Result<String, StorageError> {
        for digest in [
            &self.scope_commitment,
            &self.intent_commitment,
            &self.source_commitment,
            &self.effects_commitment,
            &self.financial_commitment,
        ] {
            records::validate_digest(digest)?;
        }
        for hash in [
            &self.candidate_digest,
            &self.signing_digest,
            &self.block_hash,
        ] {
            records::validate_hash(hash)?;
        }
        scope::decimal(&self.nonce)?;
        if self.source_identities.is_empty() || self.source_identities.len() > 32 {
            return scope::invalid("generic preparation needs bounded source identities");
        }
        let mut identities = std::collections::BTreeSet::new();
        for identity in &self.source_identities {
            records::validate_digest(identity)?;
            if !identities.insert(identity) {
                return scope::invalid("duplicate generic source identity");
            }
        }
        scope::decimal(&self.block_number)?;
        scope::bounded_json(self, scope::MAX_CONTEXT_BYTES)?;
        scope::digest(b"minutus.contract-workflow-candidate.v2", self)
    }
}

/// Temporal purpose of independently checked evidence.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StoredWorkflowEvidenceKind {
    /// Historical successful inclusion plus declared completion observations.
    Completion,
    /// Current conditions needed by one future write; never rewrites history.
    Prerequisites,
    /// Separately established final workflow goals.
    Goals,
}

/// One source-qualified observation, not an authority flag.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoredWorkflowConditionResult {
    /// Exact immutable declared condition.
    pub condition: StoredWorkflowCondition,
    /// Source/ABI-qualified canonical actual value.
    pub value: Value,
}

/// Historical receipt anchor; present only when checking an included write.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoredWorkflowReceiptAnchor {
    /// Exact signed transaction hash.
    pub transaction_hash: String,
    /// Canonical receipt block number.
    pub block_number: String,
    /// Canonical receipt block hash.
    pub block_hash: String,
    /// Verified transaction/receipt inclusion commitment.
    pub inclusion_commitment: String,
}

/// Bounded durable evidence context; it cannot recreate a fresh certificate.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoredGenericWorkflowEvidence {
    /// Temporal purpose, separate from execution state.
    pub kind: StoredWorkflowEvidenceKind,
    /// Exact immutable workflow commitment.
    pub scope_commitment: String,
    /// Exact prepared step context, absent for final goals.
    pub preparation_commitment: Option<String>,
    /// Exact transaction inclusion where required.
    pub receipt: Option<StoredWorkflowReceiptAnchor>,
    /// State cohort number for every recorded condition.
    pub state_block_number: String,
    /// State cohort identity for every recorded condition.
    pub state_block_hash: String,
    /// Independently checked source/state/condition evidence commitment.
    pub proof_commitment: String,
    /// Complete ordered declaration and actual-value pairs.
    pub results: Vec<StoredWorkflowConditionResult>,
}
impl StoredGenericWorkflowEvidence {
    /// Commits a bounded structural record; no verification is performed here.
    ///
    /// # Errors
    /// Rejects malformed fields or incompatible temporal contexts.
    pub fn commitment(&self) -> Result<String, StorageError> {
        records::validate_digest(&self.scope_commitment)?;
        records::validate_digest(&self.proof_commitment)?;
        records::validate_hash(&self.state_block_hash)?;
        scope::decimal(&self.state_block_number)?;
        if let Some(commitment) = &self.preparation_commitment {
            records::validate_digest(commitment)?;
        }
        if let Some(receipt) = &self.receipt {
            records::validate_hash(&receipt.transaction_hash)?;
            records::validate_hash(&receipt.block_hash)?;
            records::validate_digest(&receipt.inclusion_commitment)?;
            scope::decimal(&receipt.block_number)?;
            if (receipt.block_number.len(), &receipt.block_number)
                > (self.state_block_number.len(), &self.state_block_number)
            {
                return scope::invalid("workflow observation precedes its receipt");
            }
        }
        if self.results.len() > 16
            || (self.kind == StoredWorkflowEvidenceKind::Completion
                && (self.receipt.is_none() || self.preparation_commitment.is_none()))
            || (self.kind != StoredWorkflowEvidenceKind::Completion && self.receipt.is_some())
            || (self.kind == StoredWorkflowEvidenceKind::Goals
                && self.preparation_commitment.is_some())
        {
            return scope::invalid("inconsistent workflow evidence purpose");
        }
        for result in &self.results {
            scope::argument(&result.value, 0, &mut 0)?;
        }
        scope::bounded_json(self, scope::MAX_CONTEXT_BYTES)?;
        scope::digest(b"minutus.contract-workflow-evidence.v2", self)
    }
}
