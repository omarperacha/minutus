use serde::Serialize;
use serde_json::Value;

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
