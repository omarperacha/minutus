use crate::{CompilationError, CompilerBuild, compiler_build};
use serde_json::{Value, json};

#[cfg(test)]
mod tests;

/// Bounded untrusted standard-JSON source material. Construction is validation,
/// not source correspondence or signing authority.
pub struct CompilationInput {
    pub(crate) build: CompilerBuild,
    pub(crate) file: String,
    pub(crate) name: String,
    pub(crate) standard: Value,
}
impl CompilationInput {
    /// Validates a self-contained Solidity source bundle and selects the fixed
    /// output projection. URL imports, filesystem callbacks and compiler plugins
    /// are unavailable. Original code-generation settings remain unchanged.
    ///
    /// # Errors
    /// Rejects unknown compilers, absent/oversized contents, unsupported language
    /// or unsafe/incomplete source input.
    pub fn new(
        version: &str,
        file: &str,
        name: &str,
        mut standard: Value,
    ) -> Result<Self, CompilationError> {
        let build = compiler_build(version)?;
        if file.is_empty()
            || file.len() > 512
            || name.is_empty()
            || name.len() > 128
            || file.chars().chain(name.chars()).any(char::is_control)
            || standard["language"] != "Solidity"
        {
            return Err(CompilationError::InvalidInput);
        }
        let sources = standard
            .get("sources")
            .and_then(Value::as_object)
            .ok_or(CompilationError::InvalidInput)?;
        if sources.is_empty() || sources.len() > 64 || !sources.contains_key(file) {
            return Err(CompilationError::InvalidInput);
        }
        let mut total = 0usize;
        for (path, item) in sources {
            let object = item.as_object().ok_or(CompilationError::InvalidInput)?;
            let source = object
                .get("content")
                .and_then(Value::as_str)
                .ok_or(CompilationError::InvalidInput)?;
            if path.len() > 512
                || path.chars().any(char::is_control)
                || object
                    .keys()
                    .any(|key| key != "content" && key != "keccak256")
                || source.len() > 512 * 1024
            {
                return Err(CompilationError::InvalidInput);
            }
            total = total
                .checked_add(source.len())
                .ok_or(CompilationError::Limit)?;
        }
        if total > 2 * 1024 * 1024 {
            return Err(CompilationError::Limit);
        }
        let settings = standard
            .get_mut("settings")
            .and_then(Value::as_object_mut)
            .ok_or(CompilationError::InvalidInput)?;
        // This controls only output; it does not change runtime/compiler metadata.
        settings.insert("outputSelection".into(), json!({"*":{"": ["ast"],"*":["abi","metadata","storageLayout","evm.bytecode.object","evm.bytecode.linkReferences","evm.deployedBytecode.object","evm.deployedBytecode.linkReferences","evm.deployedBytecode.immutableReferences","evm.methodIdentifiers"]}}));
        if serde_json::to_vec(&standard)
            .map_err(|_| CompilationError::InvalidInput)?
            .len()
            > 4 * 1024 * 1024
        {
            return Err(CompilationError::Limit);
        }
        Ok(Self {
            build,
            file: file.into(),
            name: name.into(),
            standard,
        })
    }
    /// Host-selected official compiler build needed for this bundle.
    #[must_use]
    pub const fn compiler(&self) -> &CompilerBuild {
        &self.build
    }
}
