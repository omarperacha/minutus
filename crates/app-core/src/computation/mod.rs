//! Sandboxed computation tool contracts, project-input binding, and result handling.

mod project_inputs;
mod result;
mod tools;

pub use project_inputs::{
    ProjectComputationInputBinding, ProjectComputationInputReference,
    ProjectComputationInputSnapshot, ProjectComputationInputsError,
};
pub use result::{
    ComputationResultError, PreparedComputationResult, computation_reuse_digest,
    computation_runner_error_status, computation_terminal_status, prepare_computation_result,
};
pub use tools::{PROJECT_COMPUTATION_TOOL, PURE_COMPUTATION_TOOL, computation_tool_registry};
