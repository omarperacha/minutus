//! Unsigned contract observations with separate native outbound-data approval.
mod execution;
mod host_report;
pub(crate) use host_report::{HostContractReport, from_exchange as contract_host_report};
mod intent;
mod invocation;
mod reconciliation;
mod review;
mod schema;
mod source;
mod workflow_schema;
mod write;

pub(super) use execution::execute;
pub(super) use intent::validate_call;
pub(super) use reconciliation::{
    execute as execute_reconciliation, validate as validate_reconciliation,
};
pub(super) use schema::definitions;
pub(super) use write::{execute_write, validate_write_call};

#[cfg(test)]
mod tests;

// Schema metadata is not runtime write admission. Unpackaged unit tests inspect
// the exact production v2 schema without overriding worker/release availability.
#[cfg(test)]
pub(crate) fn declared_workflow_schema_for_tests() -> model_runtime::PromptToolDefinition {
    workflow_schema::definition()
}
