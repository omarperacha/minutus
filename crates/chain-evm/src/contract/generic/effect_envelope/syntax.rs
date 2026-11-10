use super::GenericError;
use serde_json::Value;
use std::collections::BTreeMap;

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
    pub(super) fn helper(&self, reference: i64) -> Result<&'a Value, GenericError> {
        let function = self.node(reference)?;
        if function["nodeType"] != "FunctionDefinition"
            || function["implemented"] != true
            || function["virtual"] != false
            || !matches!(
                function["visibility"].as_str(),
                Some("internal" | "private")
            )
        {
            return Err(GenericError::OpaqueEffects);
        }
        let owned = self.contracts.iter().any(|contract| {
            self.node(*contract)
                .ok()
                .and_then(|v| v["nodes"].as_array())
                .is_some_and(|nodes| {
                    nodes
                        .iter()
                        .any(|node| node["id"].as_i64() == Some(reference))
                })
        });
        if !owned {
            return Err(GenericError::OpaqueEffects);
        }
        Ok(function)
    }
}
pub(super) fn id(node: &Value) -> Result<i64, GenericError> {
    node["id"].as_i64().ok_or(GenericError::OpaqueEffects)
}
pub(super) fn reference(node: &Value) -> Result<i64, GenericError> {
    node["referencedDeclaration"]
        .as_i64()
        .ok_or(GenericError::OpaqueEffects)
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
            if nodes.insert(id, value).is_some() {
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
