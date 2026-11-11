//! Exact compiler-declared runtime substitutions, without deployment assumptions.
use crate::CompilationError;
use crate::bytecode;
use ruint::aliases::U256;
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

/// A canonical immutable word resolved through the reproduced compiler AST.
/// It describes current code; it does not certify a constructor or invariant.
pub struct ImmutableBinding {
    ast_id: i64,
    solidity_type: String,
    canonical_abi_type: String,
    value: Value,
    word: [u8; 32],
}
impl ImmutableBinding {
    /// Exact positive declaration identifier in this artifact's source AST.
    #[must_use]
    pub const fn ast_id(&self) -> i64 {
        self.ast_id
    }
    /// Compiler-resolved Solidity type; contract labels remain descriptive.
    #[must_use]
    pub fn solidity_type(&self) -> &str {
        &self.solidity_type
    }
    /// Primitive ABI representation after resolving any contract type through AST.
    #[must_use]
    pub fn canonical_abi_type(&self) -> &str {
        &self.canonical_abi_type
    }
    /// Canonical decimal integer, Boolean, address or fixed-byte value.
    #[must_use]
    pub const fn value(&self) -> &Value {
        &self.value
    }
    /// Complete observed code word; all occurrences of this declaration agree.
    #[must_use]
    pub const fn word(&self) -> &[u8; 32] {
        &self.word
    }
}
type Bindings = BTreeMap<i64, ImmutableBinding>;

pub(crate) fn match_runtime(
    template: &[u8],
    runtime: Option<&[u8]>,
    references: &Value,
    asts: &Value,
) -> Result<(Vec<u8>, Bindings), CompilationError> {
    let references = references
        .as_object()
        .ok_or(CompilationError::ArtifactMismatch)?;
    if references.len() > 128 {
        return Err(CompilationError::Limit);
    }
    if runtime.is_none() && !references.is_empty() {
        return Err(CompilationError::ArtifactMismatch);
    }
    let actual = runtime.unwrap_or(template);
    if actual.len() != template.len() {
        return Err(CompilationError::ArtifactMismatch);
    }
    let mut nodes = BTreeMap::new();
    if !references.is_empty() {
        visit(asts, 0, &mut 0, &mut nodes)?;
    }
    let mut patched = template.to_vec();
    let mut occupied = vec![false; template.len()];
    let mut bindings = BTreeMap::new();
    let mut occurrences = 0;
    for (key, ranges) in references {
        let id = key
            .parse::<i64>()
            .map_err(|_| CompilationError::ArtifactMismatch)?;
        if id <= 0 || id.to_string() != *key {
            return Err(CompilationError::ArtifactMismatch);
        }
        let (ty, abi_type) = declaration_type(
            nodes.get(&id).ok_or(CompilationError::ArtifactMismatch)?,
            &nodes,
        )?;
        let ranges = ranges
            .as_array()
            .filter(|v| !v.is_empty())
            .ok_or(CompilationError::ArtifactMismatch)?;
        let mut observed = None;
        for range in ranges {
            occurrences += 1;
            if occurrences > 512 {
                return Err(CompilationError::Limit);
            }
            let word = patch_word(template, actual, range, &mut occupied, &mut patched)?;
            if observed.is_some_and(|previous| previous != word) {
                return Err(CompilationError::ArtifactMismatch);
            }
            observed = Some(word);
        }
        let word = observed.ok_or(CompilationError::ArtifactMismatch)?;
        let value = canonical(&abi_type, word)?;
        bindings.insert(
            id,
            ImmutableBinding {
                ast_id: id,
                solidity_type: ty,
                canonical_abi_type: abi_type,
                value,
                word,
            },
        );
    }
    if patched != actual {
        return Err(CompilationError::ArtifactMismatch);
    }
    Ok((patched, bindings))
}
fn patch_word(
    template: &[u8],
    actual: &[u8],
    range: &Value,
    occupied: &mut [bool],
    patched: &mut [u8],
) -> Result<[u8; 32], CompilationError> {
    let fields = range
        .as_object()
        .ok_or(CompilationError::ArtifactMismatch)?;
    if fields.len() != 2 || range["length"].as_u64() != Some(32) {
        return Err(CompilationError::ArtifactMismatch);
    }
    let start = range["start"]
        .as_u64()
        .and_then(|v| usize::try_from(v).ok())
        .ok_or(CompilationError::ArtifactMismatch)?;
    let end = start
        .checked_add(32)
        .filter(|v| *v <= actual.len())
        .ok_or(CompilationError::ArtifactMismatch)?;
    if occupied[start..end].iter().any(|v| *v) || template[start..end].iter().any(|v| *v != 0) {
        return Err(CompilationError::ArtifactMismatch);
    }
    occupied[start..end].fill(true);
    let word: [u8; 32] = actual[start..end]
        .try_into()
        .map_err(|_| CompilationError::ArtifactMismatch)?;
    patched[start..end].copy_from_slice(&word);
    Ok(word)
}
fn declaration_type(
    node: &Value,
    nodes: &BTreeMap<i64, &Value>,
) -> Result<(String, String), CompilationError> {
    if node["nodeType"] != "VariableDeclaration"
        || node["stateVariable"] != true
        || node["mutability"] != "immutable"
    {
        return Err(CompilationError::ArtifactMismatch);
    }
    let ty = node["typeDescriptions"]["typeString"]
        .as_str()
        .ok_or(CompilationError::ArtifactMismatch)?;
    if node["typeName"]["nodeType"] == "UserDefinedTypeName" {
        let reference = node["typeName"]["referencedDeclaration"]
            .as_i64()
            .filter(|id| *id > 0)
            .ok_or(CompilationError::ArtifactMismatch)?;
        let declaration = nodes
            .get(&reference)
            .ok_or(CompilationError::ArtifactMismatch)?;
        if declaration["nodeType"] != "ContractDefinition" || !ty.starts_with("contract ") {
            return Err(CompilationError::ArtifactMismatch);
        }
        return Ok((ty.into(), "address".into()));
    }
    if node["typeName"]["nodeType"] != "ElementaryTypeName" {
        return Err(CompilationError::ArtifactMismatch);
    }
    let name = node["typeName"]["name"]
        .as_str()
        .ok_or(CompilationError::ArtifactMismatch)?;
    let canonical = ty.strip_suffix(" payable").unwrap_or(ty);
    if name != canonical
        && !(name == "uint" && canonical == "uint256")
        && !(name == "int" && canonical == "int256")
    {
        return Err(CompilationError::ArtifactMismatch);
    }
    Ok((ty.into(), canonical.into()))
}

fn width(ty: &str, prefix: &str, maximum: usize) -> Option<usize> {
    let text = ty.strip_prefix(prefix)?;
    let value = text.parse::<usize>().ok()?;
    (value > 0 && value <= maximum && value.to_string() == text).then_some(value)
}
fn canonical(ty: &str, word: [u8; 32]) -> Result<Value, CompilationError> {
    let raw = U256::from_be_bytes(word);
    let value = if ty == "bool" && raw <= U256::ONE {
        json!(raw == U256::ONE)
    } else if ty == "address" && word[..12].iter().all(|v| *v == 0) {
        json!(format!("0x{}", bytecode::encode(&word[12..])))
    } else if let Some(bits) = width(ty, "uint", 256).filter(|v| v.is_multiple_of(8)) {
        if raw.bit_len() > bits {
            return Err(CompilationError::ArtifactMismatch);
        }
        json!(raw.to_string())
    } else if let Some(bits) = width(ty, "int", 256).filter(|v| v.is_multiple_of(8)) {
        if bits < 256 {
            let mask = U256::MAX << bits;
            let expected = if raw.bit(bits - 1) { mask } else { U256::ZERO };
            if raw & mask != expected {
                return Err(CompilationError::ArtifactMismatch);
            }
        }
        if raw.bit(255) {
            let magnitude = (!raw)
                .checked_add(U256::ONE)
                .ok_or(CompilationError::ArtifactMismatch)?;
            json!(format!("-{magnitude}"))
        } else {
            json!(raw.to_string())
        }
    } else if let Some(bytes) = width(ty, "bytes", 32) {
        if word[bytes..].iter().any(|v| *v != 0) {
            return Err(CompilationError::ArtifactMismatch);
        }
        json!(format!("0x{}", bytecode::encode(&word[..bytes])))
    } else {
        return Err(CompilationError::ArtifactMismatch);
    };
    Ok(value)
}
fn visit<'a>(
    value: &'a Value,
    depth: usize,
    count: &mut usize,
    nodes: &mut BTreeMap<i64, &'a Value>,
) -> Result<(), CompilationError> {
    *count += 1;
    if depth > 64 || *count > 100_000 {
        return Err(CompilationError::Limit);
    }
    if value["nodeType"].is_string()
        && let Some(id) = value["id"].as_i64()
        && nodes.insert(id, value).is_some()
    {
        return Err(CompilationError::ArtifactMismatch);
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
pub(crate) fn commitment(bindings: &Bindings) -> Value {
    Value::Array(bindings.values().map(|binding| json!({"astId":binding.ast_id,"type":binding.solidity_type,"abiType":binding.canonical_abi_type,"value":binding.value,"word":bytecode::encode(&binding.word)})).collect())
}
