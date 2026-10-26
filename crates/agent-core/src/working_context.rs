//! Bounded working-context selection with explicit authority separation.

mod checkpoint;
mod continuity;
mod plan;
mod summary;

pub use checkpoint::SemanticCompactionCheckpoint;
pub use continuity::DialogueContinuityCheckpoint;
pub use plan::{
    WorkingContextCompactor, WorkingContextError, WorkingContextPlan, WorkingContextPolicy,
    WorkingContextRound,
};
pub use summary::{UntrustedWorkingContextSummary, WorkingContextProjection};

#[cfg(test)]
mod tests;
