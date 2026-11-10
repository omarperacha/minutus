//! Inclusion-time effect bounds from a conservative trusted compiler-AST subset.
mod expressions;
mod statements;
mod syntax;
mod verification;

use super::{GenericContractIdentity, GenericError, layout};
use crate::contract::UnsignedContractIntent;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{cell::Cell, collections::BTreeMap};
use syntax::Program;

/// An onchain-enforced source-field operation, distinct from a snapshot value.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GenericEffectOperation {
    /// Assign a constant or exact signed-calldata value.
    Set,
    /// Add the exact bounded amount with Solidity checked arithmetic.
    Increase,
    /// Subtract the exact bounded amount with Solidity checked arithmetic.
    Decrease,
}
/// Source-declared state relation proved for the exact selected invocation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenericEffectRelation {
    /// Exact source field and canonical mapping keys/array indices.
    pub path: Vec<String>,
    /// Enforced operation; no token/authority meaning is inferred.
    pub operation: GenericEffectOperation,
    /// Canonical assigned value or nonnegative checked relative amount.
    pub value: Value,
    /// Arithmetic is checked; overflowing execution reverts.
    pub checked: bool,
}
/// Universal bounded relation for this calldata within the admitted Solidity
/// grammar. This is not a proof of arbitrary business/economic semantics.
pub struct GenericEffectEnvelope {
    relations: Vec<GenericEffectRelation>,
    commitment: String,
}
impl GenericEffectEnvelope {
    /// Exact onchain-enforced relations, shown above snapshot predictions.
    #[must_use]
    pub fn relations(&self) -> &[GenericEffectRelation] {
        &self.relations
    }
    /// Compiler/source/selected-calldata and complete relation commitment.
    #[must_use]
    pub fn commitment(&self) -> &str {
        &self.commitment
    }
    pub(in crate::contract::generic) fn mapping_preimages(
        &self,
        identity: &GenericContractIdentity,
    ) -> Result<Vec<Vec<u8>>, GenericError> {
        let artifact = identity.entry_artifact()?;
        let mut preimages = Vec::new();
        for relation in &self.relations {
            for input in layout::path_preimages(artifact.storage_layout(), &relation.path)? {
                if !preimages.contains(&input) {
                    preimages.push(input);
                }
            }
        }
        Ok(preimages)
    }
}

/// Statically admits the complete selected function and modifiers before the
/// workflow's first write. No current state is needed to prove this envelope.
///
/// # Errors
/// Rejects unknown side effects, mutable dispatch, unbounded state-derived
/// effects, unsupported compiler features or exhausted analysis bounds.
pub fn admit_generic_transition(
    identity: &GenericContractIdentity,
    intent: &UnsignedContractIntent,
) -> Result<GenericEffectEnvelope, GenericError> {
    if identity.mutable_dispatch() {
        return Err(GenericError::UnsupportedProxy);
    }
    let artifact = identity.intent_artifact(intent.call().signature())?;
    let program = Program::new(artifact.source_ast(), artifact.metadata())?;
    let selector =
        alloy_primitives::hex::encode(&alloy_primitives::keccak256(intent.call().signature())[..4]);
    let function = program.function(&selector)?;
    let parameters = function["parameters"]["parameters"]
        .as_array()
        .ok_or(GenericError::OpaqueEffects)?;
    if parameters.len() != intent.call().arguments().len() {
        return Err(GenericError::InvalidAbi);
    }
    let mut values = BTreeMap::new();
    for (parameter, value) in parameters.iter().zip(intent.call().arguments()) {
        values.insert(syntax::id(parameter)?, value.clone());
    }
    let modern = modern(artifact.compiler_version())?;
    if !modern
        && (artifact.metadata()["settings"]["optimizer"]["enabled"] != false
            || artifact.metadata()["settings"]["optimizer"]
                .get("details")
                .is_some()
            || artifact.metadata()["settings"]["viaIR"] == true
            || !program.single_contract())
    {
        return Err(GenericError::OpaqueEffects);
    }
    let mut analysis = Analysis {
        program: &program,
        layout: artifact.storage_layout(),
        values,
        relations: Vec::new(),
        steps: Cell::new(0),
        modern,
        account: intent.account(),
        target: intent.call().target(),
        native_value: intent.value_wei(),
        terminated: false,
        stack: Vec::new(),
    };
    analysis.function(function, 0)?;
    if analysis.relations.is_empty() {
        return Err(GenericError::OpaqueEffects);
    }
    let material = serde_json::json!({"domain":"minutus-generic-effect-envelope-v1","source":artifact.commitment(),"function":intent.call().signature(),"arguments":intent.call().arguments(),"account":intent.account(),"target":intent.call().target(),"nativeValue":intent.value_wei(),"relations":analysis.relations});
    let commitment = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&material).map_err(|_| GenericError::OpaqueEffects)?)
    );
    Ok(GenericEffectEnvelope {
        relations: analysis.relations,
        commitment,
    })
}
struct Analysis<'a> {
    program: &'a Program<'a>,
    layout: &'a Value,
    values: BTreeMap<i64, Value>,
    relations: Vec<GenericEffectRelation>,
    steps: Cell<usize>,
    modern: bool,
    account: &'a str,
    target: &'a str,
    native_value: &'a str,
    terminated: bool,
    stack: Vec<i64>,
}
impl Analysis<'_> {
    fn step(&self) -> Result<(), GenericError> {
        let next = self.steps.get().checked_add(1).ok_or(GenericError::Limit)?;
        self.steps.set(next);
        if next > 4096 {
            Err(GenericError::Limit)
        } else {
            Ok(())
        }
    }
    fn add(
        &mut self,
        path: Vec<String>,
        operation: GenericEffectOperation,
        value: Value,
    ) -> Result<(), GenericError> {
        if self.relations.len() >= 32 || self.relations.iter().any(|old| old.path == path) {
            return Err(GenericError::OpaqueEffects);
        }
        let ty = layout::field_type(self.layout, &path)?;
        if !self.modern
            && (ty != "uint256" || operation != GenericEffectOperation::Set || path.len() != 1)
        {
            return Err(GenericError::OpaqueEffects);
        }
        let integer = ty.starts_with("uint");
        if operation != GenericEffectOperation::Set && !integer {
            return Err(GenericError::OpaqueEffects);
        }
        super::condition::validate_expected(&ty, super::GenericComparison::Equal, &value)?;
        self.relations.push(GenericEffectRelation {
            path,
            operation,
            value,
            checked: true,
        });
        Ok(())
    }
}
fn modern(version: &str) -> Result<bool, GenericError> {
    let base = version
        .split('+')
        .next()
        .ok_or(GenericError::OpaqueEffects)?;
    let parts = base.split('.').collect::<Vec<_>>();
    if parts.len() != 3 || parts[0] != "0" || parts[1] != "8" {
        return Err(GenericError::OpaqueEffects);
    }
    let patch = parts[2]
        .parse::<u32>()
        .map_err(|_| GenericError::OpaqueEffects)?;
    Ok(patch >= 38)
}

#[cfg(test)]
mod tests;
