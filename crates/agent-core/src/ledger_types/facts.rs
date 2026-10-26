use serde::{Deserialize, Serialize};

use super::{ExecutionLedgerError, FactOrigin, LedgerResource, validate_digest, validate_id};

/// Authoritative bounded values observed by a tool.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObservedValues {
    /// Lowercase content digest when the tool established one.
    pub content_sha256: Option<String>,
    /// Exact bounded match count when relevant.
    pub match_count: Option<u64>,
    /// Exact byte size when relevant.
    pub byte_size: Option<u64>,
    /// Exact line count when relevant.
    pub line_count: Option<u64>,
}

impl ObservedValues {
    pub(crate) fn validate(&self) -> Result<(), ExecutionLedgerError> {
        if let Some(digest) = &self.content_sha256 {
            validate_digest(digest)?;
        }
        if self.content_sha256.is_none()
            && self.match_count.is_none()
            && self.byte_size.is_none()
            && self.line_count.is_none()
        {
            return Err(ExecutionLedgerError::InvalidRecord(
                "observation must contain at least one authoritative value".to_owned(),
            ));
        }
        Ok(())
    }
}

/// Host input used to record one observed resource fact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewObservation {
    /// Host-generated fact ID.
    pub fact_id: String,
    /// Resolved workspace alias.
    pub workspace_alias: String,
    /// Validated workspace-relative path.
    pub relative_path: String,
    /// Opaque resource revision observed by the broker.
    pub resource_revision: String,
    /// Bounded authoritative values.
    pub values: ObservedValues,
}

/// Persisted authoritative observation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObservationFact {
    fact_id: String,
    sequence: u64,
    origin: FactOrigin,
    resource: LedgerResource,
    resource_revision: String,
    capability_revision: String,
    values: ObservedValues,
}

impl ObservationFact {
    #[must_use]
    /// Returns the stable fact ID.
    pub fn fact_id(&self) -> &str {
        &self.fact_id
    }
    #[must_use]
    /// Returns the exact fact provenance.
    pub const fn origin(&self) -> &FactOrigin {
        &self.origin
    }
    #[must_use]
    /// Returns the capability-scoped resource.
    pub const fn resource(&self) -> &LedgerResource {
        &self.resource
    }
    #[must_use]
    /// Returns the observed revision.
    pub fn resource_revision(&self) -> &str {
        &self.resource_revision
    }
    #[must_use]
    /// Returns the bounded authoritative values.
    pub const fn values(&self) -> &ObservedValues {
        &self.values
    }
    pub(crate) fn from_parts(parts: FactParts) -> Self {
        Self {
            fact_id: parts.fact_id,
            sequence: parts.sequence,
            origin: parts.origin,
            resource: parts.resource,
            resource_revision: parts.resource_revision,
            capability_revision: parts.capability_revision,
            values: parts.values,
        }
    }
    pub(crate) const fn sequence(&self) -> u64 {
        self.sequence
    }
    pub(crate) fn capability_revision(&self) -> &str {
        &self.capability_revision
    }
}

pub(crate) struct FactParts {
    pub fact_id: String,
    pub sequence: u64,
    pub origin: FactOrigin,
    pub resource: LedgerResource,
    pub resource_revision: String,
    pub capability_revision: String,
    pub values: ObservedValues,
}

/// Host input for one successfully applied resource mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewMutationChange {
    /// Resolved workspace alias.
    pub workspace_alias: String,
    /// Validated workspace-relative path.
    pub relative_path: String,
    /// Digest required before application, absent only for creation.
    pub previous_sha256: Option<String>,
    /// Authoritative digest after successful application.
    pub resulting_sha256: String,
    /// Authoritative resource revision after successful application.
    pub resource_revision: String,
}

/// One authoritative resource change from a successful mutation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MutationChange {
    resource: LedgerResource,
    previous_sha256: Option<String>,
    resulting_sha256: String,
    resource_revision: String,
    capability_revision: String,
}

impl MutationChange {
    #[must_use]
    /// Returns the changed resource.
    pub const fn resource(&self) -> &LedgerResource {
        &self.resource
    }
    #[must_use]
    /// Returns the authoritative resulting digest.
    pub fn resulting_sha256(&self) -> &str {
        &self.resulting_sha256
    }
    #[must_use]
    /// Returns the authoritative post-mutation resource revision.
    pub fn resource_revision(&self) -> &str {
        &self.resource_revision
    }
    pub(crate) fn from_parts(parts: MutationChangeParts) -> Self {
        Self {
            resource: parts.resource,
            previous_sha256: parts.previous_sha256,
            resulting_sha256: parts.resulting_sha256,
            resource_revision: parts.resource_revision,
            capability_revision: parts.capability_revision,
        }
    }
    pub(crate) fn validate(&self) -> Result<(), ExecutionLedgerError> {
        self.resource.validate()?;
        if let Some(previous) = &self.previous_sha256 {
            validate_digest(previous)?;
        }
        validate_digest(&self.resulting_sha256)?;
        validate_id("mutation resource revision", &self.resource_revision)?;
        validate_id("mutation capability revision", &self.capability_revision)
    }
}

pub(crate) struct MutationChangeParts {
    pub resource: LedgerResource,
    pub previous_sha256: Option<String>,
    pub resulting_sha256: String,
    pub resource_revision: String,
    pub capability_revision: String,
}

/// Persisted authoritative successful mutation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MutationFact {
    mutation_id: String,
    sequence: u64,
    origin: FactOrigin,
    changes: Vec<MutationChange>,
}

impl MutationFact {
    pub(crate) fn new(
        mutation_id: String,
        sequence: u64,
        origin: FactOrigin,
        changes: Vec<MutationChange>,
    ) -> Self {
        Self {
            mutation_id,
            sequence,
            origin,
            changes,
        }
    }
    #[must_use]
    /// Returns the host-generated mutation ID.
    pub fn mutation_id(&self) -> &str {
        &self.mutation_id
    }
    #[must_use]
    /// Returns the exact fact provenance.
    pub const fn origin(&self) -> &FactOrigin {
        &self.origin
    }
    #[must_use]
    /// Returns all authoritative changes.
    pub fn changes(&self) -> &[MutationChange] {
        &self.changes
    }
    pub(crate) const fn sequence(&self) -> u64 {
        self.sequence
    }
}

/// Persisted unresolved tool failure.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnresolvedFailure {
    failure_id: String,
    sequence: u64,
    origin: FactOrigin,
    code: String,
    message: String,
}

impl UnresolvedFailure {
    pub(crate) fn new(
        failure_id: String,
        sequence: u64,
        origin: FactOrigin,
        code: String,
        message: String,
    ) -> Self {
        Self {
            failure_id,
            sequence,
            origin,
            code,
            message,
        }
    }
    #[must_use]
    /// Returns the failure record ID.
    pub fn failure_id(&self) -> &str {
        &self.failure_id
    }
    #[must_use]
    /// Returns the exact request/result provenance.
    pub const fn origin(&self) -> &FactOrigin {
        &self.origin
    }
    #[must_use]
    /// Returns the stable sanitized failure code.
    pub fn code(&self) -> &str {
        &self.code
    }
    #[must_use]
    /// Returns the sanitized unresolved failure message.
    pub fn message(&self) -> &str {
        &self.message
    }
    pub(crate) const fn sequence(&self) -> u64 {
        self.sequence
    }
}

/// Persisted bounded partial outcome requiring follow-up.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PartialOutcome {
    outcome_id: String,
    sequence: u64,
    origin: FactOrigin,
    summary: String,
}

impl PartialOutcome {
    pub(crate) fn new(
        outcome_id: String,
        sequence: u64,
        origin: FactOrigin,
        summary: String,
    ) -> Self {
        Self {
            outcome_id,
            sequence,
            origin,
            summary,
        }
    }
    #[must_use]
    /// Returns the partial-outcome record ID.
    pub fn outcome_id(&self) -> &str {
        &self.outcome_id
    }
    #[must_use]
    /// Returns the exact request/result provenance.
    pub const fn origin(&self) -> &FactOrigin {
        &self.origin
    }
    #[must_use]
    /// Returns the bounded unresolved summary.
    pub fn summary(&self) -> &str {
        &self.summary
    }
    pub(crate) const fn sequence(&self) -> u64 {
        self.sequence
    }
}
