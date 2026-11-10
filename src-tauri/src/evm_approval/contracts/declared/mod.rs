//! Generic workflow integration keeps native permissions separate from declarations.
mod context;
mod execution;
mod observation;
mod report_budget;
mod reporting;
mod request;
mod scope_review;
mod signing;
mod submission;
pub(crate) use execution::{available, execute};
pub(crate) use request::DeclaredWorkflowRequest;
