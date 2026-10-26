//! Recovery, warning and typed-confirmation safeguards for wallet deletion.

mod effects;
mod error;
mod flow;
mod input;

pub use effects::{
    VaultDeletionEffectError, VaultDeletionEffects, VaultDeletionOutcome,
    VaultEncryptedMaterialRemoval, VaultRemovalStatus,
};
pub use error::VaultDeletionError;
pub use flow::{
    VAULT_DELETION_WARNING, VaultDeletionCommit, VaultDeletionPreview, VaultRecoveryConfirmation,
};
pub use input::VaultTypedConfirmation;

#[cfg(test)]
mod tests;
