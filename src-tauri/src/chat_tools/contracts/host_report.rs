//! Data-only reports from actual native workflow exchanges, never model prose.
mod display;
mod observation;
mod status;
mod workflow;
pub(crate) use workflow::{HostContractReport, from_exchange};
pub(super) const MAX_REPORT_BYTES: usize = 8 * 1024;

#[cfg(test)]
mod tests;
