//! Native secret orchestration and the sole normal application-wallet boundary.
mod commands;
mod control;
#[cfg(target_os = "macos")]
mod mutations;
#[cfg(target_os = "macos")]
pub(crate) mod native;
#[cfg(target_os = "macos")]
mod onboarding;
#[cfg(target_os = "macos")]
mod signing;
mod storage;
#[cfg(target_os = "macos")]
mod submission;
#[cfg(target_os = "macos")]
pub(crate) use submission::{WalletSigned, WalletSubmission};
mod watchdog;
pub(crate) use commands::*;
pub(crate) use control::WalletControl;
pub(crate) use watchdog::start_watchdog;

mod error;
mod execution;
#[cfg(target_os = "macos")]
mod feedback;
mod host;
mod public_metadata;
pub(crate) use error::wallet_error;
use error::{cancelled_error, password_error};
use execution::blocking;
pub(crate) use host::install_host;
use public_metadata::persist_public;
#[cfg(target_os = "macos")]
use signing::unix_now;
