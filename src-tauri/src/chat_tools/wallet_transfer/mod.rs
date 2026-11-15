//! Bounded model intent routed into the native transfer boundary.
mod execution;
mod intent;
pub(super) use execution::execute;
pub(super) use intent::{tool_definition, validate_call};
#[cfg(test)]
mod tests;
