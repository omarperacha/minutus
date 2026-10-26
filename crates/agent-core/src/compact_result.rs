//! Compact model projections backed by retained host-only tool results.

mod computation;
mod projection;
mod record;
mod source;
mod types;

pub use computation::{
    ComputationAppliedLimits, ComputationInputProvenance, ComputationPromptSafetyFinding,
    ComputationPromptSafetyTarget, ComputationProvenanceRecord, ComputationTerminalStatus,
};
pub use projection::{CompactModelSource, CompactToolResultProjection, ModelVisibleExcerpt};
pub use record::CompactToolResultRecord;
pub use source::{CompactRehydrationContext, HostSourceBinding};
pub use types::{CompactResultError, CompactResultStatus, RetainedToolPayload};

#[cfg(test)]
mod tests;
