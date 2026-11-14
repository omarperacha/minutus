use super::{records, scope};
use crate::StorageError;
use serde::{Deserialize, Serialize};

/// Bounded comparison requested of an independently verified observation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StoredPredicateComparison {
    /// Exact equality.
    Equal,
    /// A lower bound, never inferred from an event.
    AtLeast,
    /// An upper bound.
    AtMost,
}
/// Structural observation request. It is not a verified semantic effect.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum StoredObserverPredicate {
    /// Exact ERC-20 allowance under a separately qualified code/profile.
    Allowance {
        /// Exact token address.
        token: String,
        /// Exact allowance owner.
        owner: String,
        /// Exact spender.
        spender: String,
        /// Exact raw comparison amount.
        amount: String,
    },
    /// Token balance assertion, not an attribution of whole-block deltas.
    TokenBalance {
        /// Exact token address.
        token: String,
        /// Exact balance owner.
        account: String,
        /// Required bounded comparison.
        comparison: StoredPredicateComparison,
        /// Exact raw comparison amount.
        amount: String,
    },
    /// Exact word assertion under a separately qualified application layout.
    ApplicationWord {
        /// Exact application address.
        contract: String,
        /// Complete storage slot.
        slot: String,
        /// Complete expected storage word.
        value: String,
    },
}
/// Durable observer context, with no provider URL, credentials or authority flag.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoredContractObserver {
    /// Exact host profile catalogue identity.
    pub profile_id: String,
    /// Exact reviewed semantic profile revision.
    pub profile_revision: u32,
    /// Exact pinned semantic/artifact commitment.
    pub profile_commitment: String,
    /// Independently checked by the chain owner before it can satisfy a step.
    pub predicate: StoredObserverPredicate,
}
impl StoredContractObserver {
    pub(super) fn validate(&self) -> Result<(), StorageError> {
        records::validate_digest(&self.profile_commitment)?;
        if self.profile_id.is_empty()
            || self.profile_id.len() > 128
            || self.profile_revision == 0
            || !self
                .profile_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
        {
            return scope::invalid("invalid observer profile");
        }
        match &self.predicate {
            StoredObserverPredicate::Allowance {
                token,
                owner,
                spender,
                amount,
            } => {
                scope::address(token)?;
                scope::address(owner)?;
                scope::address(spender)?;
                scope::decimal(amount)
            }
            StoredObserverPredicate::TokenBalance {
                token,
                account,
                amount,
                ..
            } => {
                scope::address(token)?;
                scope::address(account)?;
                scope::decimal(amount)
            }
            StoredObserverPredicate::ApplicationWord {
                contract,
                slot,
                value,
            } => {
                scope::address(contract)?;
                records::validate_hash(slot)?;
                records::validate_hash(value)
            }
        }
    }
}

/// Fresh exact candidate context. Persisting this data grants no approval.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoredWorkflowCandidate {
    /// Immutable semantic step digest, excluding these fresh financial facts.
    pub intent_commitment: String,
    /// Exact candidate digest, also used by durable nonce ownership.
    pub candidate_digest: String,
    /// Exact independently decoded unsigned signing payload digest.
    pub signing_digest: String,
    /// Qualified source/dispatch evidence commitment, not raw source claims.
    pub source_commitment: String,
    /// Complete qualified relevant effect evidence commitment.
    pub effects_commitment: String,
    /// Exact reviewed financial facts/budget commitment.
    pub financial_commitment: String,
    /// Fresh account nonce, canonical decimal.
    pub nonce: String,
    /// Exact preparation block height.
    pub block_number: String,
    /// Exact preparation block identity.
    pub block_hash: String,
    /// Bounded postcondition/residual permission observation context.
    pub observers: Vec<StoredContractObserver>,
}
impl StoredWorkflowCandidate {
    /// Commits the complete structural context; not signing or verification.
    ///
    /// # Errors
    /// Rejects malformed commitments, context or resource bounds.
    pub fn commitment(&self) -> Result<String, StorageError> {
        self.validate()?;
        scope::digest(b"minutus.contract-workflow-candidate.v1", self)
    }
    pub(super) fn validate(&self) -> Result<(), StorageError> {
        for digest in [
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
        scope::decimal(&self.block_number)?;
        if self.observers.is_empty() || self.observers.len() > 16 {
            return scope::invalid("candidate requires bounded observer context");
        }
        for observer in &self.observers {
            observer.validate()?;
        }
        scope::bounded_json(self, scope::MAX_CONTEXT_BYTES).map(|_| ())
    }
}
