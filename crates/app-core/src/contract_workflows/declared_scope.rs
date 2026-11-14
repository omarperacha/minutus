//! Qualified graphs remain inside the exact immutable native business scope.
use super::ContractWorkflowDeclaration;
use app_storage::{StorageError, StoredWorkflowDeclaration};
use chain_evm::contract::{UnsignedContractIntent, generic::GenericContractCandidate};
use serde_json::Value;
use std::collections::BTreeSet;

pub(super) fn validate(
    declaration: &ContractWorkflowDeclaration,
    candidate: &GenericContractCandidate,
) -> Result<(), StorageError> {
    let envelope = candidate.envelope();
    let facts = ScopeFacts {
        business: candidate.business_scope().to_vec(),
        code: candidate
            .identity()
            .contracts()
            .iter()
            .map(|code| code.address.clone())
            .collect(),
        relations: envelope
            .relations()
            .iter()
            .map(|relation| {
                (
                    relation.storage_address.clone(),
                    relation.code_address.clone(),
                )
            })
            .collect(),
        invocations: envelope
            .invocations()
            .iter()
            .map(|call| InvocationFacts {
                parent: call.parent,
                storage: call.storage_address.clone(),
                code: call.code_address.clone(),
                caller: call.caller.clone(),
                function: call.function.clone(),
                arguments: call.arguments.clone(),
                value: call.value_wei.clone(),
                calldata: call.calldata.clone(),
                is_static: call.is_static,
            })
            .collect(),
    };
    validate_facts(declaration.intent(), candidate.intent(), &facts)
}

// These private projections contain no proof or signing capability. Tests may
// corrupt them to exercise the binding protocol without minting certificates.
#[derive(Clone)]
struct InvocationFacts {
    parent: Option<u16>,
    storage: String,
    code: String,
    caller: String,
    function: String,
    arguments: Vec<Value>,
    value: String,
    calldata: String,
    is_static: bool,
}
#[derive(Clone)]
struct ScopeFacts {
    business: Vec<String>,
    code: Vec<String>,
    relations: Vec<(String, String)>,
    invocations: Vec<InvocationFacts>,
}
fn validate_facts(
    declaration: &StoredWorkflowDeclaration,
    intent: &UnsignedContractIntent,
    facts: &ScopeFacts,
) -> Result<(), StorageError> {
    let declared = declaration
        .contracts
        .iter()
        .map(|contract| contract.address.as_str())
        .collect::<BTreeSet<_>>();
    let business = facts
        .business
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if business != declared || business.len() != facts.business.len() {
        return invalid("qualified business scope differs from immutable workflow");
    }
    let code = facts
        .code
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if facts.invocations.is_empty() || facts.invocations.len() > 32 || facts.relations.len() > 32 {
        return invalid("qualified workflow invocation bounds differ");
    }
    let mut readonly = Vec::with_capacity(facts.invocations.len());
    for (index, call) in facts.invocations.iter().enumerate() {
        if !declared.contains(call.storage.as_str()) || !code.contains(call.code.as_str()) {
            return invalid("qualified invocation leaves declared storage or source graph");
        }
        if index == 0 {
            if call.parent.is_some()
                || call.caller != declaration.account
                || call.storage != intent.call().target()
                || call.function != intent.call().signature()
                || call.arguments != intent.call().arguments()
                || call.value != intent.value_wei()
                || call.calldata != intent.call().calldata()
                || call.is_static
            {
                return invalid("qualified root invocation differs from immutable call");
            }
            readonly.push(false);
        } else {
            let parent_index = call
                .parent
                .map(usize::from)
                .filter(|parent| *parent < index)
                .ok_or(StorageError::InvalidRecord(
                    "qualified invocation has invalid parent",
                ))?;
            let parent = &facts.invocations[parent_index];
            let effective_static = call.is_static || readonly[parent_index];
            if call.caller != parent.storage || (effective_static && call.value != "0") {
                return invalid("qualified invocation caller or static context differs");
            }
            readonly.push(effective_static);
        }
    }
    for (storage, owner) in &facts.relations {
        if !declared.contains(storage.as_str())
            || !code.contains(owner.as_str())
            || !facts
                .invocations
                .iter()
                .any(|call| call.storage == *storage && call.code == *owner)
        {
            return invalid("qualified relation leaves declared invocation source context");
        }
    }
    Ok(())
}
fn invalid<T>(message: &'static str) -> Result<T, StorageError> {
    Err(StorageError::InvalidRecord(message))
}

#[cfg(test)]
mod tests;
