//! Versioned generic declarations. Names, predicates and source claims grant no authority.
use super::scope;
use crate::StorageError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

/// Immutable generic workflow, independent of fresh nonce, fees and evidence.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoredWorkflowDeclaration {
    /// Structural protocol version, currently two.
    pub version: u8,
    /// Exact chain identity.
    pub network: String,
    /// Host-selected canonical signing account.
    pub account: String,
    /// Exact public contracts admitted by the native outbound scope.
    pub contracts: Vec<StoredWorkflowContract>,
    /// Immutable ordered calls and their declared dependencies.
    pub steps: Vec<StoredDeclaredWorkflowStep>,
    /// Conditions checked separately after every step has completed.
    pub goals: Vec<StoredWorkflowCondition>,
}

/// A structural alias for an exact public address, never a source certificate.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoredWorkflowContract {
    /// Local immutable alias.
    pub id: String,
    /// Exact canonical address.
    pub address: String,
}

/// Exact call semantics, with historical dependencies and current requirements.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoredDeclaredWorkflowStep {
    /// Local immutable identifier.
    pub id: String,
    /// Declared contract alias.
    pub contract: String,
    /// Full ABI signature, independently matched to the qualified source.
    pub function: String,
    /// Canonical bounded ABI values.
    pub arguments: Vec<Value>,
    /// Exact native value, in wei.
    pub value_wei: String,
    /// Earlier step indices whose completion anchors must remain canonical.
    pub depends_on: Vec<u8>,
    /// Fresh conditions required before this step's signature and dispatch.
    pub prerequisites: Vec<StoredWorkflowCondition>,
    /// Conditions observed after this exact transaction is included.
    pub completion: Vec<StoredWorkflowCondition>,
}

/// Typed comparison; truth is established only by a sealed chain certificate.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StoredWorkflowComparison {
    /// Exact canonical typed equality.
    Equal,
    /// A numeric lower bound.
    AtLeast,
    /// A numeric upper bound.
    AtMost,
}

/// A declared observation, never an effect classifier or proof.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum StoredWorkflowRead {
    /// Proof-backed isolated execution of a source-qualified read function.
    AbiRead {
        /// Full signature.
        function: String,
        /// Canonical ABI arguments.
        arguments: Vec<Value>,
        /// Tuple/array indices selecting one bounded returned value.
        output_path: Vec<u16>,
    },
    /// A typed field path resolved through independently matched source layout.
    SourceField {
        /// Compiler-layout field name, followed by typed mapping/array selectors.
        path: Vec<Value>,
    },
}

/// One immutable, source-bound condition declaration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoredWorkflowCondition {
    /// Exact declared contract alias.
    pub contract: String,
    /// Typed observation request.
    pub read: StoredWorkflowRead,
    /// Required typed comparison.
    pub comparison: StoredWorkflowComparison,
    /// Canonical expected ABI value.
    pub expected: Value,
}

impl StoredWorkflowDeclaration {
    /// Validates resource bounds and immutable reference structure, not semantics.
    ///
    /// # Errors
    /// Rejects unknown versions, invalid identities/references and excess resources.
    pub fn validate_structure(&self) -> Result<(), StorageError> {
        if self.version != 2
            || self.contracts.is_empty()
            || self.contracts.len() > 32
            || self.steps.is_empty()
            || self.steps.len() > 16
            || !matches!(
                self.network.as_str(),
                "eip155:1" | "eip155:11155111" | "eip155:8453" | "eip155:84532"
            )
        {
            return scope::invalid("invalid generic workflow version/network/bounds");
        }
        scope::address(&self.account)?;
        let mut aliases = BTreeSet::new();
        let mut addresses = BTreeSet::new();
        for contract in &self.contracts {
            identifier(&contract.id)?;
            scope::address(&contract.address)?;
            if !aliases.insert(contract.id.as_str()) || !addresses.insert(contract.address.as_str())
            {
                return scope::invalid("duplicate workflow contract identity");
            }
        }
        let mut steps = BTreeSet::new();
        for (index, step) in self.steps.iter().enumerate() {
            identifier(&step.id)?;
            if !steps.insert(step.id.as_str()) || !aliases.contains(step.contract.as_str()) {
                return scope::invalid("invalid generic workflow step reference");
            }
            signature(&step.function)?;
            arguments(&step.arguments)?;
            scope::decimal(&step.value_wei)?;
            let mut dependencies = BTreeSet::new();
            if step.depends_on.len() > 15
                || step
                    .depends_on
                    .iter()
                    .any(|prior| usize::from(*prior) >= index || !dependencies.insert(*prior))
            {
                return scope::invalid("workflow dependencies must name distinct earlier steps");
            }
            conditions(&step.prerequisites, &aliases, false)?;
            conditions(&step.completion, &aliases, false)?;
            if step.completion.is_empty()
                && self.steps.iter().any(|later| {
                    later
                        .depends_on
                        .iter()
                        .any(|prior| usize::from(*prior) == index)
                })
            {
                return scope::invalid(
                    "dependent steps require declared predecessor completion conditions",
                );
            }
        }
        conditions(&self.goals, &aliases, false)?;
        scope::bounded_json(self, scope::MAX_SCOPE_BYTES).map(|_| ())
    }

    /// Domain-separated immutable declaration commitment.
    ///
    /// # Errors
    /// Rejects an invalid declaration before hashing it.
    pub fn commitment(&self) -> Result<String, StorageError> {
        self.validate_structure()?;
        scope::digest(b"minutus.contract-workflow-scope.v2", self)
    }

    /// Exact ordered step commitments, including all declared dependencies.
    ///
    /// # Errors
    /// Rejects invalid declarations or encoding failures.
    pub fn step_commitments(&self) -> Result<Vec<String>, StorageError> {
        self.validate_structure()?;
        let scope = self.commitment()?;
        self.steps
            .iter()
            .enumerate()
            .map(|(index, step)| {
                scope::digest(
                    b"minutus.contract-workflow-intent.v2",
                    &(&scope, index, step),
                )
            })
            .collect()
    }
}

fn identifier(value: &str) -> Result<(), StorageError> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        return scope::invalid("invalid workflow identifier");
    }
    Ok(())
}
fn signature(value: &str) -> Result<(), StorageError> {
    if value.is_empty()
        || value.len() > 512
        || !value.is_ascii()
        || !value.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'_' | b'(' | b')' | b'[' | b']' | b',')
        })
    {
        return scope::invalid("invalid workflow ABI signature");
    }
    Ok(())
}
fn arguments(values: &[Value]) -> Result<(), StorageError> {
    if values.len() > 16 {
        return scope::invalid("workflow ABI argument count exceeds limit");
    }
    let mut nodes = 0;
    for value in values {
        scope::argument(value, 0, &mut nodes)?;
    }
    Ok(())
}
fn conditions(
    values: &[StoredWorkflowCondition],
    aliases: &BTreeSet<&str>,
    required: bool,
) -> Result<(), StorageError> {
    if values.len() > 16 || (required && values.is_empty()) {
        return scope::invalid("workflow condition count exceeds limits");
    }
    for condition in values {
        if !aliases.contains(condition.contract.as_str()) {
            return scope::invalid("unknown condition contract");
        }
        scope::argument(&condition.expected, 0, &mut 0)?;
        match &condition.read {
            StoredWorkflowRead::AbiRead {
                function,
                arguments: args,
                output_path,
            } => {
                signature(function)?;
                arguments(args)?;
                if output_path.is_empty() || output_path.len() > 5 {
                    return scope::invalid("invalid workflow output path");
                }
            }
            StoredWorkflowRead::SourceField { path } => {
                if path.is_empty()
                    || path.len() > 8
                    || path.iter().any(|part| {
                        part.as_str()
                            .is_none_or(|part| part.is_empty() || part.len() > 256)
                    })
                {
                    return scope::invalid("invalid workflow source field path");
                }
                arguments(path)?;
            }
        }
    }
    Ok(())
}
