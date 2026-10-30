//! Fixed source-qualified workflows reuse the native signer and shared nonce owner.
mod admission;
mod audit;
mod execution;
mod observation;
mod preparation;
mod readiness;
mod reporting;
mod scope_review;
mod signing;
mod submission;

pub(crate) use execution::execute;
pub(crate) use preparation::{ContractWorkflowRequest, available};

#[cfg(test)]
mod tests;
