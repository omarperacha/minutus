//! Public source acquisition diagnostics preserve closed causes without authority.
mod compiler_cache;
mod failure;
mod http;
mod provider;
mod source_input;
pub use failure::{SourceFailure, SourceFailureReason, SourceFailureStage};
pub use provider::GenericSourceProvider;
#[cfg(test)]
mod tests;
