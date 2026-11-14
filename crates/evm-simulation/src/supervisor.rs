//! Host-only process composition; codecs and proof authority stay in their owners.
mod control;
mod exchange;
mod io;
mod local;
mod process;
mod run;
use crate::{SimulationError, StateQuery, StateValue};
pub use control::{SimulationControl, ensure_worker_runtime_available};
pub(crate) use process::WatchedProcess;
pub use run::{simulate_in_worker, simulate_with_trusted_worker};
pub(crate) use run::{supervise_offline, supervise_trusted};
/// Trusted state broker: implementations must bind every answer to the request's
/// exact block, respect `remaining` during IO and check consistency/canonicality.
/// The engine cannot supply URLs, alternate blocks, filesystem paths or sends.
pub trait SimulationStateSource {
    /// Obtains one bounded, exact-state answer under the current run deadline.
    ///
    /// # Errors
    /// Missing/inconsistent state and deadline/cancellation fail the entire run.
    fn query(
        &mut self,
        query: &StateQuery,
        control: &SimulationControl<'_>,
    ) -> Result<StateValue, SimulationError>;
}
