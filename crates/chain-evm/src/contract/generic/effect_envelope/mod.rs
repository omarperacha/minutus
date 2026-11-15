//! Inclusion-time effect bounds from a conservative trusted compiler-AST subset.
mod analysis;
mod eligibility;
mod expressions;
mod external;
mod frames;
mod graph;
mod loops;
mod statements;
mod syntax;
mod verification;

use super::{GenericContractIdentity, GenericError, layout};
use crate::contract::UnsignedContractIntent;
use analysis::Analysis;
#[cfg(test)]
use analysis::modern;
pub(super) use graph::admit as graph_admit;
use serde::Serialize;
use serde_json::Value;
#[cfg(test)]
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
    /// Canonical persistent storage owner, including fixed proxy context.
    pub storage_address: String,
    /// Reproduced code responsible for this operation.
    pub code_address: String,
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
    invocations: Vec<GenericEffectInvocation>,
    commitment: String,
}
/// One statically proved business invocation. Exact minimal-proxy forwarding
/// frames are checked separately and do not introduce another business target.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenericEffectInvocation {
    /// Parent source invocation in depth-first order; absent for the root.
    pub parent: Option<u16>,
    /// Exact persistent storage owner and business target.
    pub storage_address: String,
    /// Exact reproduced implementation, retaining proxy/code separation.
    pub code_address: String,
    /// Per-frame msg.sender, separate from the transaction origin.
    pub caller: String,
    /// Canonical compiler ABI signature.
    pub function: String,
    /// Exact bounded arguments, never snapshot-derived return values.
    pub arguments: Vec<Value>,
    /// Exact apparent native value in decimal wei.
    pub value_wei: String,
    /// Independently ABI-encoded complete calldata.
    pub calldata: String,
    /// This edge uses STATICCALL; read-only context propagates to descendants.
    pub is_static: bool,
}
/// Locally derived caller-source requirement, not callee code/effect evidence.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenericRequiredCall {
    /// Fixed business target from the hypothetical invocation operands.
    pub target: String,
    /// Compiler-resolved interface selector; inspect the target ABI separately.
    pub selector: String,
    /// Locally computed bounded arguments.
    pub arguments: Vec<Value>,
    /// Exact value for this supported edge, currently zero.
    pub value_wei: String,
    /// Source interface selects STATICCALL for this edge.
    pub is_static: bool,
}
/// Descriptive discovery for a hypothetical caller/origin. It neither fetches
/// callee code nor grants an outbound scope, complete effect proof or signer.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenericInvocationRequirements {
    /// Hypothetical transaction origin supplied to the read-only inspection.
    pub origin: String,
    /// Hypothetical msg.sender, independent of any wallet/account control.
    pub caller: String,
    /// Source-qualified entry target at the observation's historical snapshot.
    pub target: String,
    /// Canonical selected function.
    pub function: String,
    /// Canonical selected arguments.
    pub arguments: Vec<Value>,
    /// Exact hypothetical native value used by local expression analysis.
    pub value_wei: String,
    /// Direct call requirements only. Each callee needs separate inspection and
    /// explicit declaration before any future workflow source acquisition.
    pub required_calls: Vec<GenericRequiredCall>,
}

/// Derives direct call requirements locally, without fetching undeclared callees.
///
/// # Errors
/// Rejects unsupported source, mutable routing, invalid context or opaque operands.
pub fn inspect_generic_invocation(
    identity: &GenericContractIdentity,
    intent: &UnsignedContractIntent,
    origin: &str,
) -> Result<GenericInvocationRequirements, GenericError> {
    let parsed: alloy_primitives::Address = origin.parse().map_err(|_| GenericError::InvalidAbi)?;
    if format!("{parsed:#x}") != origin {
        return Err(GenericError::InvalidAbi);
    }
    if identity.entrypoint() != intent.call().target() {
        return Err(GenericError::SourceMismatch);
    }
    let target = intent
        .call()
        .target()
        .parse()
        .map_err(|_| GenericError::InvalidAbi)?;
    let (_, artifact) = identity.immutable_dispatch(target)?;
    super::abi::bind_invocation(artifact.abi(), intent, true)?;
    let result = analysis::analyze(identity, intent, origin, 0)?;
    Ok(GenericInvocationRequirements {
        origin: origin.into(),
        caller: intent.account().into(),
        target: intent.call().target().into(),
        function: intent.call().signature().into(),
        arguments: intent.call().arguments().to_vec(),
        value_wei: intent.value_wei().into(),
        required_calls: result
            .calls
            .into_iter()
            .map(|call| GenericRequiredCall {
                target: call.target,
                selector: call.selector,
                arguments: call.arguments,
                value_wei: "0".into(),
                is_static: call.is_static,
            })
            .collect(),
    })
}
impl GenericEffectEnvelope {
    /// Complete statically proved source-level call tree.
    #[must_use]
    pub fn invocations(&self) -> &[GenericEffectInvocation] {
        &self.invocations
    }
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
        let mut preimages = Vec::new();
        for relation in &self.relations {
            let address = relation
                .storage_address
                .parse()
                .map_err(|_| GenericError::OpaqueEffects)?;
            let artifact = identity
                .storage_artifact(address)
                .ok_or(GenericError::SourceMismatch)?;
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
    graph::admit(identity, intent, &[identity.entrypoint().into()])
}

/// Proves an invocation graph within explicitly declared business targets.
/// Source descriptions do not grant fresh proof or signing authority.
///
/// # Errors
/// Rejects undeclared targets, mutable routing, unknown code and opaque effects.
pub fn admit_generic_transition_in_scope(
    identity: &GenericContractIdentity,
    identities: &[&GenericContractIdentity],
    intent: &UnsignedContractIntent,
) -> Result<GenericEffectEnvelope, GenericError> {
    let merged = identity.merge_sources(identities)?;
    let mut business = identities
        .iter()
        .map(|v| v.entrypoint().to_owned())
        .collect::<Vec<_>>();
    business.push(identity.entrypoint().into());
    business.sort();
    business.dedup();
    graph::admit(&merged, intent, &business)
}

#[cfg(test)]
mod tests;
