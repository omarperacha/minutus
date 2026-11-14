//! Bounded local EVM simulation process. No networking, wallet or signing API.
//! Raw execution is evidence only; it grants no source/effect qualification.
mod admission;
pub use admission::{ReceiptGasAccounting, receipt_gas_accounting};
mod application_targets;
pub use application_targets::is_reserved_application_target;
mod contract;
mod engine;
mod native_identity;
mod native_resources;
mod worker_identity;
pub use worker_identity::{TrustedSimulationWorker, TrustedSimulationWorkers};
pub mod offline;
mod protocol;
mod replay;
mod resources;
pub use replay::{
    NativeTransfer, ReplayCall, ReplayCallKind, TrustedReplayExecution, replay_discover,
    replay_frozen,
};
mod supervisor;
pub use supervisor::{
    SimulationControl, SimulationStateSource, ensure_worker_runtime_available, simulate_in_worker,
    simulate_with_trusted_worker,
};
mod worker;

pub use contract::*;
pub use worker::run_worker;
