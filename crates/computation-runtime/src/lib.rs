//! Isolated, one-shot execution of bounded model-authored computations.
//!
//! The library owns the authenticated pipe protocol, fixed `QuickJS` policy and
//! host-side child lifecycle. Project path resolution and model-tool
//! integration remain outside this crate.

mod contract;
mod engine;
mod protocol;
mod resource_policy;
mod supervisor;

pub use contract::{
    CONTRACT_VERSION, ComputationInput, ComputationOutput, ComputationRequest, ComputationResponse,
    ContractError, InputFormat, MAX_INPUT_BYTES, MAX_INPUTS, MAX_OUTPUT_BYTES, MAX_SOURCE_BYTES,
    MAX_TOTAL_INPUT_BYTES, OutputExpectation, OutputKind, RUNTIME_VERSION, RunnerStatus,
};
pub use resource_policy::{
    CPU_TIME_LIMIT, JAVASCRIPT_HEAP_LIMIT_BYTES, JAVASCRIPT_STACK_LIMIT_BYTES,
    PROCESS_MEMORY_LIMIT_BYTES, WALL_TIME_LIMIT,
};
pub use supervisor::{ComputationCancellation, ComputationRunner, RunnerError};

/// Runs exactly one authenticated request using standard input and output.
///
/// This is the only entry point used by the bundled runner executable. It
/// returns an error without writing diagnostics when the private protocol or
/// launch credential is malformed.
///
/// # Errors
///
/// Returns a sanitized error when the launch credential, frame, request or
/// response stream is unavailable or malformed.
pub fn run_one_shot() -> Result<(), RunnerError> {
    protocol::run_one_shot(std::io::stdin().lock(), std::io::stdout().lock())
}

#[cfg(test)]
mod tests;
