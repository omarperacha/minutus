//! Fixed source-qualified workflows reuse the native signer and shared nonce owner.
mod admission;
mod audit;
mod declared;
mod execution;
mod observation;
mod preparation;
mod readiness;
mod reporting;
mod scope_review;
mod signing;
mod submission;

#[cfg(debug_assertions)]
pub(crate) use declared::qualify_preparation;
pub(crate) use declared::{
    DeclaredWorkflowRequest, available as generic_available, execute as execute_declared,
};
pub(crate) use execution::execute;
pub(crate) use preparation::{ContractWorkflowRequest, available};

#[cfg(test)]
mod tests;
