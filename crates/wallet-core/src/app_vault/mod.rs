//! One password and atomic encrypted catalogue for all application accounts.
mod catalogue;
mod document;
mod signing;
#[cfg(unix)]
mod storage;

pub use catalogue::{AppVaultEntry, AppVaultOpen};
pub use document::AppVaultDocument;
pub use signing::AppVaultFreshSigning;
#[cfg(unix)]
pub use storage::{AppVaultResetCommit, AppVaultStore};

#[cfg(test)]
mod tests;
