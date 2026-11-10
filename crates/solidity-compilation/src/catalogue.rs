use crate::CompilationError;
use serde::Deserialize;
use sha2::{Digest, Sha256};

const CATALOGUE: &str = include_str!("../compiler-catalogue.json");
// Exact published compiler build inventory, rather than contract/function pins.
const ROOT: &str = "57196e765cc87b6a82a71332fe9dcba140fd55e03000f7b3b347f4fadf818166";

#[derive(Deserialize)]
struct Catalogue {
    builds: Vec<BuildRecord>,
}
/// An official checksum-pinned compiler build; executable URLs are host-owned.
#[derive(Clone)]
pub struct CompilerBuild {
    release: String,
    long_version: String,
    path: String,
    sha256: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BuildRecord {
    version: String,
    long_version: String,
    path: String,
    sha256: String,
}
impl CompilerBuild {
    /// Complete immutable compiler version including its upstream commit.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.long_version
    }
    /// The single official public origin for this reviewed compiler artifact.
    #[must_use]
    pub fn url(&self) -> String {
        format!(
            "https://binaries.soliditylang.org/emscripten-wasm32/{}",
            self.path
        )
    }
    /// Stable cache name containing no source- or model-selected path component.
    #[must_use]
    pub fn cache_name(&self) -> &str {
        &self.path
    }
    /// Corresponding upstream source release for this exact compiler version.
    #[must_use]
    pub fn source_url(&self) -> String {
        format!(
            "https://github.com/argotorg/solidity/tree/v{}",
            self.release
        )
    }
    /// Complete licence text accompanying this external compiler program.
    #[must_use]
    pub const fn licence(&self) -> &'static str {
        include_str!("../notices/Solidity-GPL-3.0.txt")
    }
    /// Verifies public compiler bytes before caching or execution. This checksum
    /// check alone does not create a reproduced artifact.
    ///
    /// # Errors
    /// Rejects oversized data and any disagreement with the reviewed checksum.
    pub fn verify(&self, bytes: &[u8]) -> Result<(), CompilationError> {
        if bytes.is_empty() || bytes.len() > 40 * 1024 * 1024 {
            return Err(CompilationError::Limit);
        }
        if format!("0x{:x}", Sha256::digest(bytes)) != self.sha256 {
            return Err(CompilationError::CompilerMismatch);
        }
        Ok(())
    }
}
/// Selects a reviewed stable compiler build, never a URL/executable supplied by
/// the model or source provider. The full version/commit must agree exactly.
///
/// # Errors
/// Rejects modified inventories, prereleases and unsupported/mismatched builds.
pub fn compiler_build(version: &str) -> Result<CompilerBuild, CompilationError> {
    if format!("{:x}", Sha256::digest(CATALOGUE.as_bytes())) != ROOT {
        return Err(CompilationError::ArtifactMismatch);
    }
    let catalogue: Catalogue =
        serde_json::from_str(CATALOGUE).map_err(|_| CompilationError::ArtifactMismatch)?;
    catalogue
        .builds
        .into_iter()
        .find(|build| {
            // Some source services append the canonical native compiler suffix.
            version == build.long_version
                || version == format!("{}.Emscripten.clang", build.long_version)
                || version == build.version
        })
        .map(|build| CompilerBuild {
            release: build.version,
            long_version: build.long_version,
            path: build.path,
            sha256: build.sha256,
        })
        .ok_or(CompilationError::UnsupportedCompiler)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_versions_not_contracts_and_rejects_foreign_compiler_claims()
    -> Result<(), CompilationError> {
        assert_eq!(
            compiler_build("0.8.24+commit.e11b9ed9")?.version(),
            "0.8.24+commit.e11b9ed9"
        );
        assert!(compiler_build("0.8.24+commit.deadbeef").is_err());
        assert!(compiler_build("https://attacker.example/solc.js").is_err());
        assert!(compiler_build("0.8.39").is_err());
        assert!(compiler_build("0.8.38")?.verify(b"changed").is_err());
        Ok(())
    }
    #[test]
    fn canonical_full_commit_and_only_known_native_suffix_select_the_same_build()
    -> Result<(), CompilationError> {
        let bare = compiler_build("0.8.38")?;
        let exact = compiler_build(bare.version())?;
        let native = compiler_build(&format!("{}.Emscripten.clang", bare.version()))?;
        assert_eq!(bare.url(), exact.url());
        assert_eq!(bare.url(), native.url());
        for foreign in [
            format!("{} ", bare.version()),
            format!("{}.other", bare.version()),
            "0.8.38-nightly".into(),
            "0.8.038".into(),
            "file:///compiler.js".into(),
        ] {
            assert!(matches!(
                compiler_build(&foreign),
                Err(CompilationError::UnsupportedCompiler)
            ));
        }
        Ok(())
    }
}
