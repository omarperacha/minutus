mod dispatch;
#[cfg(test)]
mod tests;
use super::GenericError;
use serde_json::Value;
use std::collections::BTreeMap;

// Real declaration IDs cannot collide with the Emscripten-u32 builtin suffix.
const MAX_AST_ID: i64 = 1_000_000;
const LEGACY_ID_MODULUS: i64 = 1_i64 << 32;

pub(super) struct Program<'a> {
    nodes: BTreeMap<i64, &'a Value>,
    contracts: Vec<i64>,
}
impl<'a> Program<'a> {
    pub(super) fn single_contract(&self) -> bool {
        self.contracts.len() == 1
    }
    pub(super) fn new(asts: &'a Value, metadata: &Value) -> Result<Self, GenericError> {
        let targets = metadata["settings"]["compilationTarget"]
            .as_object()
            .filter(|v| v.len() == 1)
            .ok_or(GenericError::OpaqueEffects)?;
        let (file, name) = targets.iter().next().ok_or(GenericError::OpaqueEffects)?;
        let unit = asts.get(file).ok_or(GenericError::OpaqueEffects)?;
        let target = unit["nodes"]
            .as_array()
            .and_then(|nodes| {
                nodes
                    .iter()
                    .find(|node| node["nodeType"] == "ContractDefinition" && node["name"] == *name)
            })
            .ok_or(GenericError::OpaqueEffects)?;
        let contracts = target["linearizedBaseContracts"]
            .as_array()
            .filter(|v| v.len() <= 32)
            .ok_or(GenericError::OpaqueEffects)?
            .iter()
            .map(|v| v.as_i64().ok_or(GenericError::OpaqueEffects))
            .collect::<Result<Vec<_>, _>>()?;
        let mut nodes = BTreeMap::new();
        let mut count = 0;
        visit(asts, 0, &mut count, &mut nodes)?;
        Ok(Self { nodes, contracts })
    }
    pub(super) fn node(&self, id: i64) -> Result<&'a Value, GenericError> {
        self.nodes
            .get(&id)
            .copied()
            .ok_or(GenericError::OpaqueEffects)
    }
    pub(super) fn function(&self, selector: &str) -> Result<&'a Value, GenericError> {
        for contract in &self.contracts {
            let nodes = self.node(*contract)?["nodes"]
                .as_array()
                .ok_or(GenericError::OpaqueEffects)?;
            if let Some(function) = nodes.iter().find(|node| {
                node["nodeType"] == "FunctionDefinition"
                    && node["functionSelector"] == selector
                    && node["implemented"] == true
            }) {
                return Ok(function);
            }
        }
        Err(GenericError::OpaqueEffects)
    }
    pub(super) fn member_index(&self, member: i64) -> Result<usize, GenericError> {
        for node in self
            .nodes
            .values()
            .filter(|node| node["nodeType"] == "StructDefinition")
        {
            if let Some(members) = node["members"].as_array()
                && let Some(index) = members
                    .iter()
                    .position(|item| item["id"].as_i64() == Some(member))
            {
                return Ok(index);
            }
        }
        Err(GenericError::OpaqueEffects)
    }
}
pub(super) fn id(node: &Value) -> Result<i64, GenericError> {
    node["id"].as_i64().ok_or(GenericError::OpaqueEffects)
}
pub(super) fn reference(node: &Value) -> Result<i64, GenericError> {
    let value = node["referencedDeclaration"]
        .as_i64()
        .ok_or(GenericError::OpaqueEffects)?;
    if (0..=MAX_AST_ID).contains(&value) {
        return Ok(value);
    }
    // Official old Emscripten builds store negative magic IDs in 32-bit size_t.
    // Accept only the exact reviewed builtin name/ID pairs, never a name alone.
    let builtin = match node["name"].as_str() {
        Some("assert") => -3,
        Some("msg") => -15,
        Some("require") => -18,
        Some("revert") => -19,
        Some("super") => -25,
        Some("tx") => -26,
        Some("this") => -28,
        _ => return Err(GenericError::OpaqueEffects),
    };
    if value == builtin || value == LEGACY_ID_MODULUS + builtin {
        Ok(builtin)
    } else {
        Err(GenericError::OpaqueEffects)
    }
}
fn visit<'a>(
    value: &'a Value,
    depth: usize,
    count: &mut usize,
    nodes: &mut BTreeMap<i64, &'a Value>,
) -> Result<(), GenericError> {
    *count += 1;
    if *count > 100_000 || depth > 64 {
        return Err(GenericError::Limit);
    }
    if let Some(kind) = value["nodeType"].as_str() {
        if let Some(id) = value["id"].as_i64() {
            if !(0..=MAX_AST_ID).contains(&id) || nodes.insert(id, value).is_some() {
                return Err(GenericError::OpaqueEffects);
            }
        } else if !kind.starts_with("Yul") {
            return Err(GenericError::OpaqueEffects);
        }
    }
    match value {
        Value::Object(object) => {
            for child in object.values() {
                visit(child, depth + 1, count, nodes)?;
            }
        }
        Value::Array(array) => {
            for child in array {
                visit(child, depth + 1, count, nodes)?;
            }
        }
        _ => {}
    }
    Ok(())
}
