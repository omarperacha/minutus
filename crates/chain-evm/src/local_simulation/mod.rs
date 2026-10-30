//! Exact-state local execution; RPC header identity remains provider-trusted.
mod context;
mod execution;
mod frozen;
pub(crate) use frozen::{ProvedBaseSnapshot, freeze_proved_base};
mod fees;
mod header;
mod observation;
mod proof;
mod provenance;
pub(crate) use observation::read_proved;
mod record;
pub(crate) use provenance::{canonical_header, latest_header};
mod source;

pub(crate) use execution::run_proved;
pub use execution::simulate_contract_with_worker;
pub use fees::{SimulationFeeFacts, contract_fee_facts};
pub(crate) use record::{ProvedSimulation, ProvedState};

#[cfg(test)]
mod tests;
