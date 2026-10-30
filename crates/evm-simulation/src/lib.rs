//! Bounded local EVM simulation process. No networking, wallet or signing API.
//! Raw execution is evidence only; it grants no source/effect qualification.
mod admission;
mod contract;
mod engine;
mod native_identity;
mod native_resources;
mod worker_identity;
pub use worker_identity::{TrustedSimulationWorker, TrustedSimulationWorkers};
pub mod offline;
mod protocol;
mod resources;
mod supervisor;
pub use supervisor::{
    SimulationControl, SimulationStateSource, simulate_in_worker, simulate_with_trusted_worker,
};
mod worker;

pub use contract::*;
pub use worker::run_worker;
