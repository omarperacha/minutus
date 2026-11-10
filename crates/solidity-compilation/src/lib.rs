//! Exact local Solidity reproduction. Source-provider claims cannot mint an ABI.
//! The compiler and its `JavaScriptCore` host are explicit trusted components;
//! untrusted source is compiled in a separately bounded, authority-free process.
mod artifact;
mod catalogue;
mod input;
mod native_identity;
mod supervisor;

pub use artifact::ReproducedArtifact;
pub use catalogue::{CompilerBuild, compiler_build};
pub use input::CompilationInput;
pub use supervisor::TrustedCompiler;

use std::{error::Error, fmt};

/// Bounded source/compiler qualification failures, without source/provider text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompilationError {
    /// Unsupported compiler build; no caller-selected executable is accepted.
    UnsupportedCompiler,
    /// Missing, altered or indirect packaged compiler helper.
    WorkerUnavailable,
    /// Official compiler bytes disagree with their reviewed checksum.
    CompilerMismatch,
    /// Unbounded/malformed source input or unsupported compiler settings.
    InvalidInput,
    /// Compiler rejected the source, or its output is invalid/incomplete.
    CompilationFailed,
    /// Compiler/artifact/input correspondence is inconsistent.
    ArtifactMismatch,
    /// Fixed memory, output, CPU, source or wall-time bound exceeded.
    Limit,
    /// Cancelled before a complete reproduced artifact existed.
    Cancelled,
}
impl fmt::Display for CompilationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Local source reproduction failed: {self:?}")
    }
}
impl Error for CompilationError {}
