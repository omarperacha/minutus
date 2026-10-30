//! Bounded structural intent records. These values never qualify source/effects.
use crate::StorageError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub(super) const MAX_SCOPE_BYTES: usize = 64 * 1024;
pub(super) const MAX_CONTEXT_BYTES: usize = 16 * 1024;

/// Immutable semantic scope, excluding block, nonce, gas, fees and approval.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoredWorkflowScope {
    /// Structural format version; unknown versions fail closed.
    pub version: u8,
    /// Exact supported CAIP-2 identity, shared by every sequential step.
    pub network: String,
    /// Canonical public signing account, shared by every step.
    pub account: String,
    /// Fixed ordered semantic intents; no automatic quote substitution.
    pub steps: Vec<StoredWorkflowIntent>,
}

/// One fixed semantic call template. Profile labels are references, not trust.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoredWorkflowIntent {
    /// Canonical application target.
    pub target: String,
    /// Host-selected pinned profile identity; never a verified-source claim.
    pub profile_id: String,
    /// Exact reviewed profile revision.
    pub profile_revision: u32,
    /// Exact pinned artifact/profile commitment, independent of its label.
    pub profile_commitment: String,
    /// Full canonical ABI function signature; the application revalidates ABI.
    pub operation: String,
    /// Canonical typed JSON values; no object, numeric-coercion or null values.
    pub arguments: Vec<Value>,
    /// Exact canonical decimal native value.
    pub value_wei: String,
}

impl StoredWorkflowScope {
    /// Checks shape and allocation bounds before an application ABI parser runs.
    /// It does not establish ABI, source, effect or approval semantics.
    ///
    /// # Errors
    /// Rejects unsupported versions, malformed fields and resource excess.
    pub fn validate_structure(&self) -> Result<(), StorageError> {
        self.validate()
    }

    /// Computes a domain-separated commitment to the complete immutable scope.
    ///
    /// # Errors
    /// Rejects unsupported structural versions, noncanonical values or limits.
    pub fn commitment(&self) -> Result<String, StorageError> {
        self.validate()?;
        digest(b"minutus.contract-workflow-scope.v1", self)
    }
    /// Ordered semantic commitments, each also bound to chain and account.
    ///
    /// # Errors
    /// Rejects malformed or oversized structural scope.
    pub fn step_commitments(&self) -> Result<Vec<String>, StorageError> {
        self.validate()?;
        self.steps
            .iter()
            .map(|step| {
                digest(
                    b"minutus.contract-workflow-intent.v1",
                    &(&self.network, &self.account, step),
                )
            })
            .collect()
    }
    pub(super) fn validate(&self) -> Result<(), StorageError> {
        if self.version != 1
            || !matches!(
                self.network.as_str(),
                "eip155:1" | "eip155:11155111" | "eip155:8453" | "eip155:84532"
            )
            || self.steps.is_empty()
            || self.steps.len() > 16
        {
            return invalid("invalid workflow scope version/network/step count");
        }
        address(&self.account)?;
        for step in &self.steps {
            address(&step.target)?;
            super::records::validate_digest(&step.profile_commitment)?;
            decimal(&step.value_wei)?;
            if step.profile_id.is_empty()
                || step.profile_id.len() > 128
                || step.profile_revision == 0
                || !step
                    .profile_id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
                || step.operation.is_empty()
                || step.operation.len() > 512
                || !step.operation.is_ascii()
                || !step.operation.bytes().all(|b| {
                    b.is_ascii_alphanumeric()
                        || matches!(b, b'_' | b'(' | b')' | b'[' | b']' | b',')
                })
                || step.arguments.len() > 16
            {
                return invalid("invalid workflow intent/profile");
            }
            let mut nodes = 0;
            for value in &step.arguments {
                argument(value, 0, &mut nodes)?;
            }
        }
        bounded_json(self, MAX_SCOPE_BYTES).map(|_| ())
    }
}

fn argument(value: &Value, depth: usize, nodes: &mut usize) -> Result<(), StorageError> {
    *nodes += 1;
    if depth > 4 || *nodes > 256 {
        return invalid("workflow argument tree exceeds limits");
    }
    match value {
        Value::String(text) if text.len() <= 2050 => Ok(()),
        Value::Bool(_) => Ok(()),
        Value::Array(items) if items.len() <= 32 => {
            for item in items {
                argument(item, depth + 1, nodes)?;
            }
            Ok(())
        }
        _ => invalid("invalid workflow argument value"),
    }
}

pub(super) fn address(value: &str) -> Result<(), StorageError> {
    if value.len() != 42
        || !value.starts_with("0x")
        || !value[2..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || value[2..].bytes().all(|b| b == b'0')
    {
        return invalid("invalid canonical workflow address");
    }
    Ok(())
}
pub(super) fn decimal(value: &str) -> Result<(), StorageError> {
    const MAX: &str =
        "115792089237316195423570985008687907853269984665640564039457584007913129639935";
    if value.is_empty()
        || value.len() > MAX.len()
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|b| b.is_ascii_digit())
        || (value.len() == MAX.len() && value > MAX)
    {
        return invalid("invalid canonical workflow quantity");
    }
    Ok(())
}
pub(super) fn bounded_json(value: &impl Serialize, max: usize) -> Result<String, StorageError> {
    let json = serde_json::to_string(value)
        .map_err(|_| StorageError::InvalidRecord("invalid workflow JSON"))?;
    if json.len() > max {
        return invalid("workflow record exceeds size limit");
    }
    Ok(json)
}
pub(super) fn digest(domain: &[u8], value: &impl Serialize) -> Result<String, StorageError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| StorageError::InvalidRecord("invalid workflow commitment input"))?;
    let mut hash = Sha256::new();
    hash.update(domain);
    hash.update([0]);
    hash.update(bytes);
    Ok(format!("{:x}", hash.finalize()))
}
pub(super) fn invalid<T>(message: &'static str) -> Result<T, StorageError> {
    Err(StorageError::InvalidRecord(message))
}
