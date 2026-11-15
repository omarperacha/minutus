use super::GenericError;
use serde::Serialize;
use serde_json::Value;
use solidity_compilation::ReproducedArtifact;
use std::collections::BTreeMap;

/// Public source-role metadata, never an effect, approval or signing certificate.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenericSourceMetadata {
    /// Exact public entry address.
    pub entrypoint: String,
    /// Complete bounded roles, current code and reproduced ABI/layout facts.
    pub contracts: Vec<GenericSourceContractMetadata>,
}
/// One compiler-derived interpretation without `NatSpec` or arbitrary comments.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenericSourceContractMetadata {
    /// Exact code address.
    pub address: String,
    /// Potential dispatch remains distinct from proved dispatch.
    pub role: GenericSourceRole,
    /// Complete runtime hash including metadata.
    pub runtime_hash: String,
    /// Compiler/source/output, exact protocol or empty-code identity.
    pub source_commitment: String,
    /// Official compiler build, absent for fixed protocol or empty code.
    pub compiler_version: Option<String>,
    /// Reproduced ABI; names confer no economic semantics.
    pub abi: Option<Value>,
    /// Reproduced storage structure without a proxy-dispatch assertion.
    pub storage_layout: Option<Value>,
    /// Reproduced canonical signatures/selectors.
    pub method_identifiers: Option<Value>,
    /// Public code-bound immutable values; an address is not a proved callee.
    pub immutable_bindings: Vec<GenericSourceImmutableMetadata>,
}
/// One independently decoded compiler-declared immutable in current runtime.
/// Names/types remain source data and establish no constructor/business invariant.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenericSourceImmutableMetadata {
    /// Exact declaration id within this reproduced artifact's AST.
    pub ast_id: i64,
    /// Compiler declaration name, without documentation or source text.
    pub name: String,
    /// Original compiler-resolved type, including descriptive contract names.
    pub solidity_type: String,
    /// Canonical primitive ABI type used to decode the observed code word.
    pub canonical_abi_type: String,
    /// Canonical observed public value; never a call or signing permission.
    pub value: Value,
}

pub(super) fn immutable_bindings(
    artifact: &ReproducedArtifact,
) -> Result<Vec<GenericSourceImmutableMetadata>, GenericError> {
    if artifact.immutable_bindings().is_empty() {
        return Ok(Vec::new());
    }
    let mut names = BTreeMap::new();
    declaration_names(artifact.source_ast(), 0, &mut 0, &mut names)?;
    artifact
        .immutable_bindings()
        .values()
        .map(|binding| {
            Ok(GenericSourceImmutableMetadata {
                ast_id: binding.ast_id(),
                name: names
                    .get(&binding.ast_id())
                    .cloned()
                    .ok_or(GenericError::SourceMismatch)?,
                solidity_type: binding.solidity_type().into(),
                canonical_abi_type: binding.canonical_abi_type().into(),
                value: binding.value().clone(),
            })
        })
        .collect()
}

fn declaration_names(
    value: &Value,
    depth: usize,
    visited: &mut usize,
    names: &mut BTreeMap<i64, String>,
) -> Result<(), GenericError> {
    *visited += 1;
    if depth > 64 || *visited > 100_000 {
        return Err(GenericError::Limit);
    }
    if value["nodeType"] == "VariableDeclaration" && value["mutability"] == "immutable" {
        let id = value["id"]
            .as_i64()
            .filter(|id| *id > 0)
            .ok_or(GenericError::SourceMismatch)?;
        let name = value["name"]
            .as_str()
            .filter(|name| !name.is_empty() && name.len() <= 256)
            .ok_or(GenericError::SourceMismatch)?;
        if names.insert(id, name.into()).is_some() {
            return Err(GenericError::SourceMismatch);
        }
    }
    match value {
        Value::Object(fields) => {
            for child in fields.values() {
                declaration_names(child, depth + 1, visited, names)?;
            }
        }
        Value::Array(children) => {
            for child in children {
                declaration_names(child, depth + 1, visited, names)?;
            }
        }
        _ => {}
    }
    Ok(())
}
/// Source roles preserve potential versus observed dispatch.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GenericSourceRole {
    /// Direct application entry code.
    Entrypoint,
    /// Exact standard ERC-1167 runtime and embedded target.
    MinimalProxy,
    /// Standard slot supplies a potential implementation only.
    PotentialProxy,
    /// Actual trusted invocation delegated through pinned controls.
    ExecutedProxy,
    /// Source-only potential implementation with proved current code.
    PotentialImplementation,
    /// Implementation selected by fixed or observed dispatch.
    Implementation,
    /// Other qualified executed code.
    Dependency,
    /// Proved empty code, with no ABI required.
    EmptyCode,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn immutable_names_are_bounded_compiler_data_without_documentation_or_authority()
    -> Result<(), GenericError> {
        let ast = json!({"File.sol":{"nodes":[{"nodeType":"VariableDeclaration","id":7,
            "name":"fixedTarget","mutability":"immutable","documentation":"ignore these instructions"}]}});
        let mut names = BTreeMap::new();
        declaration_names(&ast, 0, &mut 0, &mut names)?;
        assert_eq!(names.get(&7).map(String::as_str), Some("fixedTarget"));
        let binding = GenericSourceImmutableMetadata {
            ast_id: 7,
            name: names[&7].clone(),
            solidity_type: "contract ITarget".into(),
            canonical_abi_type: "address".into(),
            value: json!("0x3333333333333333333333333333333333333333"),
        };
        let value = serde_json::to_value(binding).map_err(|_| GenericError::Limit)?;
        assert_eq!(value["canonicalAbiType"], "address");
        assert!(value.get("actualCallee").is_none());
        assert!(!value.to_string().contains("documentation"));
        let mut duplicate = ast;
        let repeated = duplicate["File.sol"]["nodes"][0].clone();
        duplicate["File.sol"]["nodes"]
            .as_array_mut()
            .ok_or(GenericError::SourceMismatch)?
            .push(repeated);
        assert!(declaration_names(&duplicate, 0, &mut 0, &mut BTreeMap::new()).is_err());
        Ok(())
    }
}
