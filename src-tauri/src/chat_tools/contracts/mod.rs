//! Unsigned contract observations with separate native outbound-data approval.
mod execution;
mod intent;
mod review;
mod schema;
mod write;

pub(super) use execution::execute;
pub(super) use intent::validate_call;
pub(super) use schema::definitions;
pub(super) use write::{execute_write, validate_write_call};

#[cfg(test)]
mod tests;
