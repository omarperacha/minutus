//! Trusted transfer approval boundary; authority stays in the owning modules.
pub(crate) mod contracts;
mod execution;
#[cfg(target_os = "macos")]
mod native;
pub(crate) mod presentation;
mod recovery;
mod registry;
pub(crate) use execution::{review_chat_transfer, review_evm_transaction};
pub(crate) use recovery::reconcile_prior_transfers;
pub(crate) use registry::{ApprovalCandidate, ApprovalOrigin, EvmApprovalControl};
#[cfg(test)]
mod tests;

#[cfg(all(debug_assertions, target_os = "macos"))]
mod preview;
#[cfg(all(debug_assertions, target_os = "macos"))]
pub(crate) use preview::run as preview_contract_approval;
