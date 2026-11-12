//! Generic workflow integration keeps native permissions separate from declarations.
mod context;
mod execution;
mod lifecycle;
mod observation;
#[cfg(debug_assertions)]
mod qualification;
#[cfg(debug_assertions)]
pub(crate) use qualification::run as qualify_preparation;
mod qualification_stage;
mod report_budget;
mod reporting;
mod request;
mod scope_review;
mod signing;
mod submission;
pub(crate) use execution::{available, execute};
pub(crate) use request::DeclaredWorkflowRequest;
