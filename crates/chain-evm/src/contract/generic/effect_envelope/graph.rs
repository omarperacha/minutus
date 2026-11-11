//! Source-first, IO-free call graph proof; observed frames only cross-check it.
use super::{
    GenericContractIdentity, GenericEffectEnvelope, GenericEffectInvocation, GenericEffectRelation,
    GenericError, analysis, external::ExternalCall,
};
use crate::contract::{DeclaredContractCall, UnsignedContractIntent, generic::abi};
use alloy_primitives::Address;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub(in crate::contract::generic) fn admit(
    identity: &GenericContractIdentity,
    intent: &UnsignedContractIntent,
    business: &[String],
) -> Result<GenericEffectEnvelope, GenericError> {
    let scope = business
        .iter()
        .map(|v| v.parse())
        .collect::<Result<BTreeSet<Address>, _>>()
        .map_err(|_| GenericError::InvalidAbi)?;
    if scope.is_empty() || scope.len() != business.len() || scope.len() > 16 {
        return Err(GenericError::Limit);
    }
    let canonical_scope = scope
        .iter()
        .map(|address| format!("{address:#x}"))
        .collect::<Vec<_>>();
    let mut graph = Graph {
        identity,
        origin: intent.account(),
        business: scope,
        invocations: Vec::new(),
        relations: Vec::new(),
        active: BTreeSet::new(),
        steps: 0,
    };
    graph.walk(intent, None, false, false, 0)?;
    let material = serde_json::json!({"domain":"minutus-effect-graph-v2","source":identity.commitment(),"scope":canonical_scope,"invocations":graph.invocations,"relations":graph.relations});
    let commitment = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&material).map_err(|_| GenericError::OpaqueEffects)?)
    );
    Ok(GenericEffectEnvelope {
        relations: graph.relations,
        invocations: graph.invocations,
        commitment,
    })
}
struct Graph<'a> {
    identity: &'a GenericContractIdentity,
    origin: &'a str,
    business: BTreeSet<Address>,
    invocations: Vec<GenericEffectInvocation>,
    relations: Vec<GenericEffectRelation>,
    active: BTreeSet<(Address, String)>,
    steps: usize,
}
impl Graph<'_> {
    fn walk(
        &mut self,
        intent: &UnsignedContractIntent,
        parent: Option<u16>,
        is_static: bool,
        inherited_static: bool,
        depth: usize,
    ) -> Result<(), GenericError> {
        let target: Address = intent
            .call()
            .target()
            .parse()
            .map_err(|_| GenericError::InvalidAbi)?;
        if !self.business.contains(&target) {
            return Err(GenericError::SourceMismatch);
        }
        if depth >= 8 || self.invocations.len() >= 32 {
            return Err(GenericError::Limit);
        }
        let key = (target, intent.call().signature().to_owned());
        if !self.active.insert(key.clone()) {
            return Err(GenericError::OpaqueEffects);
        }
        let (code, artifact) = self.identity.immutable_dispatch(target)?;
        if reserved_target(intent.network(), target) || reserved_target(intent.network(), code) {
            return Err(GenericError::OpaqueEffects);
        }
        abi::bind_invocation(artifact.abi(), intent, parent.is_some())?;
        let index = u16::try_from(self.invocations.len()).map_err(|_| GenericError::Limit)?;
        self.invocations.push(GenericEffectInvocation {
            parent,
            storage_address: intent.call().target().into(),
            code_address: format!("{code:#x}"),
            caller: intent.account().into(),
            function: intent.call().signature().into(),
            arguments: intent.call().arguments().to_vec(),
            value_wei: intent.value_wei().into(),
            calldata: intent.call().calldata().into(),
            is_static,
        });
        let result = analysis::analyze(self.identity, intent, self.origin, self.steps)?;
        self.steps = result.steps;
        let readonly = is_static || inherited_static;
        if readonly && !result.relations.is_empty() {
            return Err(GenericError::OpaqueEffects);
        }
        for relation in result.relations {
            // A final snapshot cannot establish intermediate alias ordering.
            // Every persistent path has exactly one source-level writer.
            if self.relations.len() >= 32
                || self.relations.iter().any(|old| overlaps(old, &relation))
            {
                return Err(GenericError::OpaqueEffects);
            }
            self.relations.push(relation);
        }
        for call in result.calls {
            let child = self.child(intent, &call)?;
            self.walk(&child, Some(index), call.is_static, readonly, depth + 1)?;
        }
        self.active.remove(&key);
        Ok(())
    }
    fn child(
        &self,
        parent: &UnsignedContractIntent,
        call: &ExternalCall,
    ) -> Result<UnsignedContractIntent, GenericError> {
        let target: Address = call.target.parse().map_err(|_| GenericError::InvalidAbi)?;
        if !self.business.contains(&target) {
            return Err(GenericError::SourceMismatch);
        }
        let (_, artifact) = self.identity.immutable_dispatch(target)?;
        let methods = artifact
            .methods()
            .as_object()
            .ok_or(GenericError::InvalidAbi)?;
        let selected = methods
            .iter()
            .filter(|(_, v)| v.as_str() == Some(&call.selector))
            .collect::<Vec<_>>();
        if selected.len() != 1 {
            return Err(GenericError::InvalidAbi);
        }
        let signature = selected[0].0;
        let function = artifact
            .abi()
            .as_array()
            .ok_or(GenericError::InvalidAbi)?
            .iter()
            .find(|f| f["type"] == "function" && abi::signature(f).is_ok_and(|s| s == *signature))
            .ok_or(GenericError::InvalidAbi)?;
        let outputs = abi::parameters(&function["outputs"])?;
        let declared =
            DeclaredContractCall::new(&call.target, signature, &call.arguments, &outputs)
                .map_err(|_| GenericError::InvalidAbi)?;
        UnsignedContractIntent::new_declared(
            parent.network(),
            parent.call().target(),
            "0",
            declared,
        )
        .map_err(|_| GenericError::InvalidAbi)
    }
}
fn reserved_target(network: crate::EvmNetwork, address: Address) -> bool {
    let network = match network {
        crate::EvmNetwork::Mainnet => evm_simulation::SimulationNetwork::BaseMainnet,
        crate::EvmNetwork::Sepolia => evm_simulation::SimulationNetwork::BaseSepolia,
        crate::EvmNetwork::EthereumMainnet => evm_simulation::SimulationNetwork::EthereumMainnet,
        crate::EvmNetwork::EthereumSepolia => evm_simulation::SimulationNetwork::EthereumSepolia,
    };
    evm_simulation::is_reserved_application_target(network, address.into_array())
}

fn overlaps(a: &GenericEffectRelation, b: &GenericEffectRelation) -> bool {
    a.storage_address == b.storage_address
        && (a.path.starts_with(&b.path) || b.path.starts_with(&a.path))
}
