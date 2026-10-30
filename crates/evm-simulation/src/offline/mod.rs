//! Host-only frozen Base execution. Preparing state has a broker; execution does
//! not. Signed bytes and raw child output must never enter model/IPC diagnostics.
mod database;
mod envelope;
mod fees;
mod result;
mod snapshot;
mod transport;
mod worker;

pub use fees::BaseFeeCeiling;
pub use result::{OfflineExecution, OfflineMaterialExpectation, ValidatedOfflineExecution};
pub use snapshot::FrozenBaseRequest;
pub use snapshot::base_native_manifest;
pub use transport::{diagnose_base_offline, execute_base_offline};
pub use worker::run_offline_worker;

const MAX_ENVELOPE: usize = 8_512;

#[cfg(test)]
mod profile_tests;
#[cfg(test)]
mod tests;
