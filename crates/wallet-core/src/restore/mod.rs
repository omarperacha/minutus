//! Local recovery-phrase validation, EVM preview and explicit restore commit gate.

mod error;
mod flow;
mod input;
mod inventory;

pub use error::WalletRestoreError;
pub use flow::{
    ConfirmedWalletRestore, RestoredEvmAddress, WalletRestoreChoice, WalletRestoreCommit,
    WalletRestorePreview,
};
pub use input::RecoveryPhraseInput;
pub use inventory::{MAX_LOCAL_VAULTS, VaultInventory};

#[cfg(test)]
mod tests;
