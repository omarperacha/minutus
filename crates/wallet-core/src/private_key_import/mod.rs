//! EVM private-key validation, address preview and recovery-warning gate.

mod error;
mod flow;
mod input;
mod key;

pub use error::EvmPrivateKeyImportError;
pub use flow::{
    EvmPrivateKeyImportCommit, EvmPrivateKeyImportPreview, EvmPrivateKeyRecoveryAcknowledgement,
    IMPORTED_KEY_RECOVERY_WARNING,
};
pub use input::EvmPrivateKeyInput;
pub use key::ImportedEvmPrivateKey;

#[cfg(test)]
mod tests;
