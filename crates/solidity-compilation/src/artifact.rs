use crate::{CompilationError, CompilationInput, ImmutableBinding, immutables};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

/// Reproduced compiler output minted only by the trusted local compiler owner.
/// It has no deserialisation or caller-supplied-success constructor. Correspondence
/// with current proved onchain code remains the chain owner's separate duty.
pub struct ReproducedArtifact {
    abi: Value,
    storage_layout: Value,
    runtime: Vec<u8>,
    creation: Vec<u8>,
    metadata: Value,
    compilation_settings: Value,
    methods: Value,
    source_ast: Value,
    compiler_version: String,
    commitment: String,
    immutable_bindings: BTreeMap<i64, ImmutableBinding>,
}
impl ReproducedArtifact {
    pub(crate) fn from_compiler(
        input: &CompilationInput,
        output: &Value,
    ) -> Result<Self, CompilationError> {
        Self::build(input, output, None)
    }
    pub(crate) fn for_runtime(
        input: &CompilationInput,
        output: &Value,
        runtime: &[u8],
    ) -> Result<Self, CompilationError> {
        Self::build(input, output, Some(runtime))
    }
    fn build(
        input: &CompilationInput,
        output: &Value,
        matched_runtime: Option<&[u8]>,
    ) -> Result<Self, CompilationError> {
        if output
            .get("errors")
            .and_then(Value::as_array)
            .is_some_and(|errors| errors.iter().any(|error| error["severity"] == "error"))
        {
            return Err(CompilationError::CompilationFailed);
        }
        let artifact = output
            .get("contracts")
            .and_then(|v| v.get(&input.file))
            .and_then(|v| v.get(&input.name))
            .ok_or(CompilationError::CompilationFailed)?;
        let evm = &artifact["evm"];
        for reference in [
            &evm["deployedBytecode"]["linkReferences"],
            &evm["bytecode"]["linkReferences"],
        ] {
            if !reference.as_object().is_some_and(serde_json::Map::is_empty) {
                return Err(CompilationError::ArtifactMismatch);
            }
        }
        let runtime = crate::bytecode::decode(
            evm["deployedBytecode"]["object"]
                .as_str()
                .ok_or(CompilationError::CompilationFailed)?,
        )?;
        let creation = crate::bytecode::decode(
            evm["bytecode"]["object"]
                .as_str()
                .ok_or(CompilationError::CompilationFailed)?,
        )?;
        if runtime.is_empty() || runtime.len() > 24_576 || creation.len() > 49_152 {
            return Err(CompilationError::Limit);
        }
        let abi = artifact
            .get("abi")
            .filter(|v| v.as_array().is_some_and(|a| a.len() <= 256))
            .ok_or(CompilationError::CompilationFailed)?
            .clone();
        let storage_layout = artifact
            .get("storageLayout")
            .filter(|v| v.is_object())
            .ok_or(CompilationError::CompilationFailed)?
            .clone();
        let methods = evm
            .get("methodIdentifiers")
            .filter(|v| v.is_object())
            .ok_or(CompilationError::CompilationFailed)?
            .clone();
        let metadata: Value = serde_json::from_str(
            artifact["metadata"]
                .as_str()
                .ok_or(CompilationError::CompilationFailed)?,
        )
        .map_err(|_| CompilationError::CompilationFailed)?;
        if metadata["compiler"]["version"] != input.build.version()
            || metadata["output"]["abi"] != abi
            || metadata["settings"]["compilationTarget"][&input.file] != input.name
        {
            return Err(CompilationError::ArtifactMismatch);
        }
        let source_ast = source_asts(input, output)?;
        let (runtime, immutable_bindings) = immutables::match_runtime(
            &runtime,
            matched_runtime,
            &evm["deployedBytecode"]["immutableReferences"],
            &source_ast,
        )?;
        let commitment =
            artifact_commitment(input, artifact, &source_ast, &runtime, &immutable_bindings)?;
        Ok(Self {
            abi,
            storage_layout,
            runtime,
            creation,
            metadata,
            compilation_settings: input.standard["settings"].clone(),
            methods,
            source_ast,
            compiler_version: input.build.version().into(),
            commitment,
            immutable_bindings,
        })
    }
    /// ABI produced by the exact compiler, not a provider-declared ABI.
    #[must_use]
    pub const fn abi(&self) -> &Value {
        &self.abi
    }
    /// Reproduced source-declared storage structure; names do not prove economic semantics.
    #[must_use]
    pub const fn storage_layout(&self) -> &Value {
        &self.storage_layout
    }
    /// Complete matched runtime, with exact metadata and declared immutable words.
    #[must_use]
    pub fn runtime(&self) -> &[u8] {
        &self.runtime
    }
    /// Typed words observed only at exact compiler-declared immutable references.
    /// These bindings establish no constructor execution or creation invariant.
    #[must_use]
    pub const fn immutable_bindings(&self) -> &BTreeMap<i64, ImmutableBinding> {
        &self.immutable_bindings
    }
    /// Complete reproduced creation program; generic admission need not assume deployment invariants.
    #[must_use]
    pub fn creation(&self) -> &[u8] {
        &self.creation
    }
    /// Exact compiler-produced metadata.
    #[must_use]
    pub const fn metadata(&self) -> &Value {
        &self.metadata
    }
    /// Exact validated standard-JSON input settings, already bound by commitment.
    /// Metadata can omit historical code-generation details and cannot replace
    /// these original settings when assessing compiler feature eligibility.
    #[must_use]
    pub const fn compilation_settings(&self) -> &Value {
        &self.compilation_settings
    }
    /// Compiler-produced canonical function signatures and selectors.
    #[must_use]
    pub const fn methods(&self) -> &Value {
        &self.methods
    }
    /// Compiler-resolved source-unit ASTs keyed by source path, used only by
    /// bounded structural effect verification, never provider-supplied ASTs.
    #[must_use]
    pub const fn source_ast(&self) -> &Value {
        &self.source_ast
    }
    /// Official compiler version and build commit.
    #[must_use]
    pub fn compiler_version(&self) -> &str {
        &self.compiler_version
    }
    /// Domain-separated input, compiler and complete output binding.
    #[must_use]
    pub fn commitment(&self) -> &str {
        &self.commitment
    }
}
fn source_asts(input: &CompilationInput, output: &Value) -> Result<Value, CompilationError> {
    let expected = input.standard["sources"]
        .as_object()
        .ok_or(CompilationError::ArtifactMismatch)?;
    let sources = output["sources"]
        .as_object()
        .ok_or(CompilationError::ArtifactMismatch)?;
    if sources.len() != expected.len() || sources.keys().any(|name| !expected.contains_key(name)) {
        return Err(CompilationError::ArtifactMismatch);
    }
    sources
        .iter()
        .map(|(name, item)| {
            Ok((
                name.clone(),
                item["ast"]
                    .as_object()
                    .ok_or(CompilationError::ArtifactMismatch)?
                    .clone()
                    .into(),
            ))
        })
        .collect::<Result<serde_json::Map<String, Value>, CompilationError>>()
        .map(Value::Object)
}

fn artifact_commitment(
    input: &CompilationInput,
    artifact: &Value,
    source_ast: &Value,
    runtime: &[u8],
    bindings: &BTreeMap<i64, ImmutableBinding>,
) -> Result<String, CompilationError> {
    let mut material = serde_json::json!({"domain":"minutus-reproduced-solidity-v1",
        "compiler":input.build.version(),"input":input.standard,
        "file":input.file,"name":input.name,"artifact":artifact,"sourceAst":source_ast});
    // Preserve existing non-immutable commitments; actual substitutions extend
    // the binding only when constructor-free observed words are present.
    if !bindings.is_empty() {
        material["runtime"] = crate::bytecode::encode(runtime).into();
        material["immutableBindings"] = immutables::commitment(bindings);
    }
    Ok(format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&material).map_err(|_| CompilationError::ArtifactMismatch)?
        )
    ))
}
