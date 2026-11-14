use evm_simulation::SimulationError;
use std::{error::Error, fmt};

/// Closed admission failures; no model/provider prose or secret-bearing payloads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QualificationError {
    /// Exact canonical function, arguments/value or source ABI differ.
    InvalidIntent,
    /// No reviewed direct code family matches, including unknown proxy dispatch.
    UnsupportedProfile,
    /// Bundled compiler/source/artifact correspondence changed.
    InvalidArtifact,
    /// Exact successful canonical deployment correspondence is missing/invalid.
    InvalidDeployment,
    /// A required account/slot was not proved at the exact snapshot.
    MissingState,
    /// Request, chain, account, code, block or before-state disagree.
    SnapshotMismatch,
    /// Failed, truncated, reverted, inconsistent or incomplete execution.
    IncompleteSimulation,
    /// Uncovered calls, logs, storage, native balance or nonce changes.
    UncoveredEffects,
    /// Required exact arithmetic, balance or allowance bound failed.
    Arithmetic,
    /// Profiled approval requires zero-first or explicit revocation.
    ApprovalRace,
    /// Current execution/fee protocol does not qualify this network.
    UnsupportedNetwork,
    /// Bounded local execution/state failure.
    Simulation(SimulationError),
}
impl fmt::Display for QualificationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Contract admission rejected: {self:?}")
    }
}
impl Error for QualificationError {}
impl From<SimulationError> for QualificationError {
    fn from(value: SimulationError) -> Self {
        Self::Simulation(value)
    }
}
