//! Encrypted wallet vault, key lifecycle and isolated signing boundary.
//!
//! Secret-bearing operations stay inside this crate. The vault format exposes
//! only encrypted payload bytes and explicitly public, authenticated account
//! metadata.

mod account_derivation;
mod evm_address;
mod evm_signing;
mod private_key_import;
mod recovery_phrase;
mod restore;
mod vault_crypto;
mod vault_format;
mod vault_key;
mod vault_store;
mod wallet_deletion;
mod wallet_lock;

mod app_vault;
mod password_vault;

#[cfg(unix)]
pub use password_vault::PasswordVaultStore;
pub use password_vault::{
    MAX_PASSWORD_VAULT_BYTES, MAX_WALLET_PASSWORD_BYTES, MIN_WALLET_PASSWORD_CHARACTERS,
    PASSWORD_VAULT_VERSION, PasswordSigningAttempt, PasswordVaultDocument, PasswordVaultError,
    PasswordVaultJob, PasswordVaultUnlock, PasswordVaultWork, WalletPassword,
};

pub use account_derivation::{
    DEFAULT_EVM_ACCOUNT_RECOVERY_GUIDANCE, DefaultEvmAccount, EVM_DEFAULT_DERIVATION_PATH,
    EvmAccountDerivationError, derive_default_evm_account,
};
pub use evm_address::EvmAddress;
#[cfg(target_os = "macos")]
pub use evm_signing::sign_evm_with_mac_os_user_presence;
pub use evm_signing::{
    EvmSigningError, EvmSigningKey, EvmSigningObservation, EvmTransactionSignature,
};
pub use private_key_import::{
    EvmPrivateKeyImportCommit, EvmPrivateKeyImportError, EvmPrivateKeyImportPreview,
    EvmPrivateKeyInput, EvmPrivateKeyRecoveryAcknowledgement, IMPORTED_KEY_RECOVERY_WARNING,
    ImportedEvmPrivateKey,
};
pub use recovery_phrase::{
    RECOVERY_ENTROPY_BYTES, RECOVERY_PHRASE_WORDS, RECOVERY_VERIFICATION_WARNINGS,
    RECOVERY_VERIFICATION_WORDS, RecoveryPhrase, RecoveryPhraseError, RecoverySeed,
    RecoveryVerificationPrompt, RecoveryVerificationWarnings, RecoveryWarningAcknowledgement,
    RecoveryWordAnswers, RecoveryWordPosition,
};
pub use restore::{
    ConfirmedWalletRestore, MAX_LOCAL_VAULTS, RecoveryPhraseInput, RestoredEvmAddress,
    VaultInventory, WalletRestoreChoice, WalletRestoreCommit, WalletRestoreError,
    WalletRestorePreview,
};

#[cfg(target_os = "macos")]
pub use vault_key::MacOsVaultKeyStore;
pub use vault_key::{
    VAULT_ENCRYPTION_KEY_BYTES, VaultEncryptionKey, VaultKeyId, VaultKeyStoreError,
    VaultSigningAuthorization, VaultUnlockAuthorization,
};
pub use vault_store::{EncryptedVaultStore, PreparedVaultWrite, VaultStoreError};
pub use wallet_deletion::{
    VAULT_DELETION_WARNING, VaultDeletionCommit, VaultDeletionEffectError, VaultDeletionEffects,
    VaultDeletionError, VaultDeletionOutcome, VaultDeletionPreview, VaultEncryptedMaterialRemoval,
    VaultRecoveryConfirmation, VaultRemovalStatus, VaultTypedConfirmation,
};
pub use wallet_lock::{
    MAX_INACTIVITY_TIMEOUT, MIN_INACTIVITY_TIMEOUT, VaultInactivityTimeout, VaultLockReason,
    VaultSession, VaultSessionError, VaultSessionStatus, VaultSigningPermit,
};

pub use vault_crypto::{
    VaultCryptoError, open_vault_signing_key, seal_imported_private_key_vault,
    seal_recovery_seed_vault,
};
pub use vault_format::{
    AUTHENTICATION_TAG_BYTES, AccountKeyOrigin, AccountMetadata, CURRENT_VAULT_VERSION,
    ENCRYPTION_ALGORITHM, KDF_ALGORITHM, KDF_INFO, KDF_SALT_BYTES, MAX_ACCOUNTS,
    MAX_ENCRYPTED_PAYLOAD_BYTES, NONCE_BYTES, VaultDocument, VaultFormatError,
};

pub use app_vault::{AppVaultDocument, AppVaultEntry, AppVaultFreshSigning, AppVaultOpen};
#[cfg(unix)]
pub use app_vault::{AppVaultResetCommit, AppVaultStore};
