//! One compiler-resolved invocation, with state-independent material operands.
use super::{
    GenericContractIdentity, GenericEffectOperation, GenericEffectRelation, GenericError,
    external::ExternalCall,
    layout,
    syntax::{self, Program},
};
use crate::contract::UnsignedContractIntent;
use serde_json::Value;
use solidity_compilation::ImmutableBinding;
use std::{cell::Cell, collections::BTreeMap};

pub(super) struct Analysis<'a> {
    pub(super) program: &'a Program<'a>,
    pub(super) layout: &'a Value,
    pub(super) values: BTreeMap<i64, Value>,
    pub(super) relations: Vec<GenericEffectRelation>,
    pub(super) calls: Vec<ExternalCall>,
    pub(super) steps: Cell<usize>,
    pub(super) modern: bool,
    pub(super) features: super::eligibility::Features,
    pub(super) base: bool,
    pub(super) account: &'a str,
    pub(super) origin: &'a str,
    pub(super) target: &'a str,
    pub(super) code_address: &'a str,
    pub(super) immutable_bindings: &'a BTreeMap<i64, ImmutableBinding>,
    pub(super) native_value: &'a str,
    pub(super) terminated: bool,
    pub(super) stack: Vec<i64>,
}
pub(super) struct AnalysisResult {
    pub(super) relations: Vec<GenericEffectRelation>,
    pub(super) calls: Vec<ExternalCall>,
    pub(super) steps: usize,
}

pub(super) fn analyze(
    identity: &GenericContractIdentity,
    intent: &UnsignedContractIntent,
    origin: &str,
    steps: usize,
) -> Result<AnalysisResult, GenericError> {
    let target = intent
        .call()
        .target()
        .parse()
        .map_err(|_| GenericError::InvalidAbi)?;
    let (code, artifact) = identity.immutable_dispatch(target)?;
    let code = format!("{code:#x}");
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
    let features = super::eligibility::admit(artifact, &program)?;
    // Eligibility is feature-based: reviewed historical scalar programs may
    // use the structured grammar without inheriting modern aggregate support.
    let modern = true;
    let mut analysis = Analysis {
        program: &program,
        layout: artifact.storage_layout(),
        values,
        relations: Vec::new(),
        calls: Vec::new(),
        steps: Cell::new(steps),
        modern,
        features,
        base: intent.network().is_base(),
        account: intent.account(),
        origin,
        target: intent.call().target(),
        code_address: &code,
        immutable_bindings: artifact.immutable_bindings(),
        native_value: intent.value_wei(),
        terminated: false,
        stack: Vec::new(),
    };
    analysis.function(function, 0)?;
    Ok(AnalysisResult {
        relations: analysis.relations,
        calls: analysis.calls,
        steps: analysis.steps.get(),
    })
}

impl Analysis<'_> {
    pub(super) fn step(&self) -> Result<(), GenericError> {
        let next = self.steps.get().checked_add(1).ok_or(GenericError::Limit)?;
        self.steps.set(next);
        if next > 4096 {
            Err(GenericError::Limit)
        } else {
            Ok(())
        }
    }
    pub(super) fn add(
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
        if operation != GenericEffectOperation::Set && !ty.starts_with("uint") {
            return Err(GenericError::OpaqueEffects);
        }
        super::super::condition::validate_expected(
            &ty,
            super::super::GenericComparison::Equal,
            &value,
        )?;
        self.relations.push(GenericEffectRelation {
            storage_address: self.target.into(),
            code_address: self.code_address.into(),
            path,
            operation,
            value,
            checked: true,
        });
        Ok(())
    }
}
pub(super) fn modern(version: &str) -> Result<bool, GenericError> {
    let parts = version
        .split('+')
        .next()
        .ok_or(GenericError::OpaqueEffects)?
        .split('.')
        .collect::<Vec<_>>();
    if parts.len() != 3 || parts[0] != "0" || parts[1] != "8" {
        return Err(GenericError::OpaqueEffects);
    }
    Ok(parts[2]
        .parse::<u32>()
        .map_err(|_| GenericError::OpaqueEffects)?
        >= 38)
}
