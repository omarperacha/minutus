//! Host-selected live snapshots retain the selection clock through every proof.
use super::GenericError;
use super::completion_diagnostics::{self as diagnostics, Stage};
use crate::{EvmNetwork, EvmRpcUrl, local_simulation};
use evm_simulation::SimulationControl;
use std::{
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

/// Process-local latest selection. Historical hashes and persisted metadata
/// cannot construct this capability or restart its monotonic freshness clock.
pub struct GenericLiveCohort {
    pub(super) network: EvmNetwork,
    pub(super) block_hash: String,
    pub(super) block_number: u64,
    pub(super) selected_at: Instant,
}
impl GenericLiveCohort {
    /// Exact host-selected network.
    #[must_use]
    pub const fn network(&self) -> EvmNetwork {
        self.network
    }
    /// Exact canonical selection identity.
    #[must_use]
    pub fn block_hash(&self) -> &str {
        &self.block_hash
    }
    /// Exact selection height.
    #[must_use]
    pub const fn block_number(&self) -> u64 {
        self.block_number
    }
    /// Age starts before latest-header acquisition, never after proof work.
    #[must_use]
    pub fn is_fresh(&self) -> bool {
        self.selected_at.elapsed() < Duration::from_secs(30)
    }
    pub(super) fn retained(&self) -> Self {
        Self {
            network: self.network,
            block_hash: self.block_hash.clone(),
            block_number: self.block_number,
            selected_at: self.selected_at,
        }
    }
    pub(super) fn check(
        &self,
        endpoint: &EvmRpcUrl,
        control: &SimulationControl<'_>,
    ) -> Result<(), GenericError> {
        if !self.is_fresh() {
            return Err(GenericError::Expired);
        }
        canonical_live_anchor(
            endpoint,
            self.network,
            &self.block_hash,
            self.block_number,
            control,
        )?;
        if !self.is_fresh() {
            return Err(GenericError::Expired);
        }
        Ok(())
    }
}
/// Selects one latest canonical cohort with an immutable acquisition-start clock.
/// The caller must already hold native outbound consent.
///
/// # Errors
/// Rejects unavailable/noncanonical state, cancellation and expired acquisition.
pub fn select_generic_live_cohort(
    endpoint: &EvmRpcUrl,
    network: EvmNetwork,
    cancelled: &AtomicBool,
) -> Result<GenericLiveCohort, GenericError> {
    let selected_at = Instant::now();
    let control = SimulationControl::start(cancelled);
    let header = diagnostics::simulation(Stage::ConditionCohortSelection, &control, || {
        local_simulation::latest_header(endpoint, network, &control)
    })
    .map_err(|_| GenericError::SnapshotMismatch)?;
    let cohort = GenericLiveCohort {
        network,
        block_hash: format!("{:#x}", header.hash_slow()),
        block_number: header.number,
        selected_at,
    };
    if !cohort.is_fresh() {
        return Err(GenericError::Expired);
    }
    Ok(cohort)
}
// Canonicality is checked after sampling the head as well: a reorganisation
// during that sample must not turn an old anchor into fresh current evidence.
pub(super) fn canonical_live_anchor(
    endpoint: &EvmRpcUrl,
    network: EvmNetwork,
    hash: &str,
    number: u64,
    control: &SimulationControl<'_>,
) -> Result<(), GenericError> {
    let head = diagnostics::simulation(Stage::LiveHeadRecheck, control, || {
        local_simulation::latest_header(endpoint, network, control)
    })
    .map_err(|_| GenericError::SnapshotMismatch)?;
    let anchor = diagnostics::simulation(Stage::LiveAnchorRecheck, control, || {
        local_simulation::canonical_header(endpoint, network, hash, control)
    })
    .map_err(|_| GenericError::SnapshotMismatch)?;
    check_heights(number, anchor.number, head.number)
}
fn check_heights(expected: u64, anchor: u64, head: u64) -> Result<(), GenericError> {
    if anchor != expected || head < expected {
        Err(GenericError::SnapshotMismatch)
    } else {
        Ok(())
    }
}
#[cfg(test)]
mod tests;
