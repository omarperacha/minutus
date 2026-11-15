//! Bounded state-independent local control, never an arbitrary EVM interpreter.
use super::{Analysis, GenericError, expressions, syntax};
use alloy_primitives::{I256, U256};
use serde_json::{Value, json};

#[cfg(test)]
mod tests;

impl Analysis<'_> {
    pub(super) fn for_statement(&mut self, node: &Value, depth: usize) -> Result<(), GenericError> {
        if !self.modern
            || self.features != super::eligibility::Features::Structured
            || node["condition"].is_null()
        {
            return Err(GenericError::OpaqueEffects);
        }
        let initial = &node["initializationExpression"];
        if !initial.is_null() {
            self.statement(initial, depth + 1)?;
        }
        for iteration in 0..=32 {
            self.step()?;
            let condition = self
                .value(&node["condition"], 0)?
                .as_bool()
                .ok_or(GenericError::OpaqueEffects)?;
            if !condition {
                return Ok(());
            }
            if iteration == 32 {
                return Err(GenericError::Limit);
            }
            self.statement(&node["body"], depth + 1)?;
            if self.terminated {
                return Ok(());
            }
            let update = &node["loopExpression"];
            if !update.is_null() {
                self.statement(update, depth + 1)?;
            }
        }
        Err(GenericError::Limit)
    }
    pub(super) fn local_declaration(&mut self, node: &Value) -> Result<(), GenericError> {
        let declaration = node["declarations"]
            .as_array()
            .filter(|v| v.len() == 1)
            .and_then(|v| v.first())
            .ok_or(GenericError::OpaqueEffects)?;
        if declaration["storageLocation"] == "storage" || declaration["stateVariable"] == true {
            return Err(GenericError::OpaqueEffects);
        }
        let ty = expressions::base_type(declaration)?;
        let value = if node["initialValue"].is_null() {
            if !self.modern
                || self.features != super::eligibility::Features::Structured
                || !ty.starts_with("uint")
            {
                return Err(GenericError::OpaqueEffects);
            }
            json!("0")
        } else {
            self.value(&node["initialValue"], 0)?
        };
        expressions::canonical(ty, &value)?;
        self.values.insert(syntax::id(declaration)?, value);
        Ok(())
    }
    /// Returns false only for a storage/nonlocal lvalue, whose existing owner
    /// continues admission. No aggregate alias or state value enters this map.
    pub(super) fn local_mutation(&mut self, node: &Value) -> Result<bool, GenericError> {
        let left = match node["nodeType"].as_str() {
            Some("Assignment") => &node["leftHandSide"],
            Some("UnaryOperation") if matches!(node["operator"].as_str(), Some("++" | "--")) => {
                &node["subExpression"]
            }
            _ => return Ok(false),
        };
        if left["nodeType"] != "Identifier" {
            return Ok(false);
        }
        let id = syntax::reference(left)?;
        let Some(old) = self.values.get(&id).cloned() else {
            return Ok(false);
        };
        if !self.modern || self.features != super::eligibility::Features::Structured {
            return Err(GenericError::OpaqueEffects);
        }
        let declaration = self.program.node(id)?;
        let ty = expressions::base_type(declaration)?;
        if declaration["nodeType"] != "VariableDeclaration"
            || declaration["stateVariable"] == true
            || declaration["storageLocation"] == "storage"
            || declaration["typeDescriptions"]["typeString"]
                .as_str()
                .is_none_or(|value| value.contains('['))
            || !primitive(ty)
        {
            return Err(GenericError::OpaqueEffects);
        }
        let operator = node["operator"]
            .as_str()
            .ok_or(GenericError::OpaqueEffects)?;
        let value = if operator == "=" {
            self.value(&node["rightHandSide"], 0)?
        } else {
            let rhs = if matches!(operator, "++" | "--") {
                json!("1")
            } else {
                self.value(&node["rightHandSide"], 0)?
            };
            arithmetic(ty, operator, &old, &rhs)?
        };
        expressions::canonical(ty, &value)?;
        self.values.insert(id, value);
        Ok(true)
    }
}
fn primitive(ty: &str) -> bool {
    if matches!(ty, "bool" | "address" | "contract") {
        return true;
    }
    for prefix in ["uint", "int"] {
        if let Some(width) = ty.strip_prefix(prefix) {
            return width.parse::<u16>().is_ok_and(|bits| {
                (8..=256).contains(&bits) && bits % 8 == 0 && bits.to_string() == width
            });
        }
    }
    ty.strip_prefix("bytes").is_some_and(|width| {
        width
            .parse::<u8>()
            .is_ok_and(|bytes| (1..=32).contains(&bytes) && bytes.to_string() == width)
    })
}

fn arithmetic(ty: &str, operator: &str, old: &Value, rhs: &Value) -> Result<Value, GenericError> {
    let old = old.as_str().ok_or(GenericError::OpaqueEffects)?;
    let rhs = rhs.as_str().ok_or(GenericError::OpaqueEffects)?;
    if ty.starts_with("uint") {
        let old = U256::from_str_radix(old, 10).map_err(|_| GenericError::OpaqueEffects)?;
        let rhs = U256::from_str_radix(rhs, 10).map_err(|_| GenericError::OpaqueEffects)?;
        let value = match operator {
            "++" | "+=" => old.checked_add(rhs),
            "--" | "-=" => old.checked_sub(rhs),
            "*=" => old.checked_mul(rhs),
            "/=" => old.checked_div(rhs),
            "%=" => old.checked_rem(rhs),
            _ => return Err(GenericError::OpaqueEffects),
        }
        .ok_or(GenericError::OpaqueEffects)?;
        Ok(json!(value.to_string()))
    } else if ty.starts_with("int") {
        let old = old
            .parse::<I256>()
            .map_err(|_| GenericError::OpaqueEffects)?;
        let rhs = rhs
            .parse::<I256>()
            .map_err(|_| GenericError::OpaqueEffects)?;
        let value = match operator {
            "++" | "+=" => old.checked_add(rhs),
            "--" | "-=" => old.checked_sub(rhs),
            "*=" => old.checked_mul(rhs),
            "/=" => old.checked_div(rhs),
            "%=" => old.checked_rem(rhs),
            _ => return Err(GenericError::OpaqueEffects),
        }
        .ok_or(GenericError::OpaqueEffects)?;
        Ok(json!(value.to_string()))
    } else {
        Err(GenericError::OpaqueEffects)
    }
}
