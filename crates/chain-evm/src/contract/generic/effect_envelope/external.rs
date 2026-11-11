//! Only compiler-resolved high-level calls with fixed operands enter the graph.
use super::{Analysis, GenericError, syntax};
use alloy_primitives::Address;
use serde_json::Value;

pub(super) struct ExternalCall {
    pub(super) target: String,
    pub(super) selector: String,
    pub(super) arguments: Vec<Value>,
    pub(super) is_static: bool,
}
impl Analysis<'_> {
    pub(super) fn external_call(&mut self, call: &Value) -> Result<(), GenericError> {
        self.step()?;
        let member = &call["expression"];
        if !self.modern
            || call["kind"] != "functionCall"
            || member["nodeType"] != "MemberAccess"
            || call["names"].as_array().is_none_or(|v| !v.is_empty())
            || self.calls.len() >= 32
        {
            return Err(GenericError::OpaqueEffects);
        }
        let function = self.program.node(syntax::reference(member)?)?;
        if function["nodeType"] != "FunctionDefinition"
            || !matches!(function["visibility"].as_str(), Some("public" | "external"))
        {
            return Err(GenericError::OpaqueEffects);
        }
        if self.features == super::eligibility::Features::LegacyScalar {
            super::eligibility::external_abi(function)?;
        }
        let selector = function["functionSelector"]
            .as_str()
            .filter(|v| v.len() == 8 && v.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or(GenericError::OpaqueEffects)?;
        let target = self.value(&member["expression"], 0)?;
        let target: Address = target
            .as_str()
            .ok_or(GenericError::OpaqueEffects)?
            .parse()
            .map_err(|_| GenericError::OpaqueEffects)?;
        let arguments = call["arguments"]
            .as_array()
            .filter(|v| v.len() <= 16)
            .ok_or(GenericError::OpaqueEffects)?
            .iter()
            .map(|v| self.value(v, 0))
            .collect::<Result<Vec<_>, _>>()?;
        self.calls.push(ExternalCall {
            target: format!("{target:#x}"),
            selector: selector.into(),
            arguments,
            is_static: matches!(function["stateMutability"].as_str(), Some("pure" | "view")),
        });
        Ok(())
    }
}
