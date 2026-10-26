//! Authenticated encryption and opening of wallet-owned secret vault payloads.

use std::{error::Error, fmt};

use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{AeadInPlace as _, KeyInit as _},
};
use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::{
    AUTHENTICATION_TAG_BYTES, AccountMetadata, EvmPrivateKeyImportCommit, EvmSigningError,
    EvmSigningKey, ImportedEvmPrivateKey, KDF_INFO, KDF_SALT_BYTES, NONCE_BYTES, RecoverySeed,
    VaultDocument, VaultEncryptionKey, VaultFormatError, VaultKeyId,
};

const SECRET_SCHEMA_VERSION: u8 = 1;
const RECOVERY_SEED_KIND: u8 = 1;
const IMPORTED_PRIVATE_KEY_KIND: u8 = 2;
const RECOVERY_SEED_BYTES: usize = 64;
const PRIVATE_KEY_BYTES: usize = 32;

/// Closed failures for authenticated wallet-vault encryption and opening.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VaultCryptoError {
    /// Operating-system randomness was unavailable.
    RandomnessUnavailable,
    /// The versioned public vault envelope was invalid.
    InvalidDocument,
    /// Key derivation or authenticated encryption failed.
    CryptographicFailure,
    /// The decrypted secret payload did not match the closed schema.
    InvalidSecretPayload,
    /// The decrypted key did not control the authenticated public account.
    AccountMismatch,
    /// The decrypted EVM signing key could not be constructed.
    InvalidSigningKey,
}

impl fmt::Display for VaultCryptoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::RandomnessUnavailable => "wallet encryption randomness is unavailable",
            Self::InvalidDocument => "encrypted wallet document is invalid",
            Self::CryptographicFailure => "encrypted wallet authentication failed",
            Self::InvalidSecretPayload => "encrypted wallet payload is invalid",
            Self::AccountMismatch => "encrypted wallet key does not match its public account",
            Self::InvalidSigningKey => "encrypted wallet signing key is invalid",
        })
    }
}

impl Error for VaultCryptoError {}

impl From<VaultFormatError> for VaultCryptoError {
    fn from(_: VaultFormatError) -> Self {
        Self::InvalidDocument
    }
}

impl From<EvmSigningError> for VaultCryptoError {
    fn from(_: EvmSigningError) -> Self {
        Self::InvalidSigningKey
    }
}

/// Encrypts a verified recovery seed into a versioned authenticated vault.
///
/// # Errors
///
/// Returns a closed crypto or format error when randomness, derivation,
/// encryption, metadata validation or document construction fails.
pub fn seal_recovery_seed_vault(
    key: &VaultEncryptionKey,
    seed: &RecoverySeed,
    accounts: Vec<AccountMetadata>,
) -> Result<VaultDocument, VaultCryptoError> {
    let mut secret = Zeroizing::new(Vec::with_capacity(2 + RECOVERY_SEED_BYTES));
    secret.extend_from_slice(&[SECRET_SCHEMA_VERSION, RECOVERY_SEED_KIND]);
    secret.extend_from_slice(seed.expose_secret());
    seal(key.expose_secret(), secret, accounts)
}

/// Encrypts a confirmed imported private key into an authenticated vault.
///
/// # Errors
///
/// Returns a closed crypto or format error when randomness, encryption,
/// metadata validation or document construction fails.
pub fn seal_imported_private_key_vault(
    key: &VaultEncryptionKey,
    commit: EvmPrivateKeyImportCommit,
    accounts: Vec<AccountMetadata>,
) -> Result<VaultDocument, VaultCryptoError> {
    let private_key = commit.into_private_key();
    let mut secret = Zeroizing::new(Vec::with_capacity(2 + PRIVATE_KEY_BYTES));
    secret.extend_from_slice(&[SECRET_SCHEMA_VERSION, IMPORTED_PRIVATE_KEY_KIND]);
    secret.extend_from_slice(private_key.expose_secret());
    seal(key.expose_secret(), secret, accounts)
}

/// Authenticates and opens one vault directly into its wallet-owned signer.
///
/// # Errors
///
/// Returns a closed error for invalid authenticated metadata, a wrong key,
/// malformed secret payload, unsupported payload kind or account mismatch.
pub fn open_vault_signing_key(
    document: &VaultDocument,
    key: &[u8; 32],
    vault_id: VaultKeyId,
) -> Result<EvmSigningKey, VaultCryptoError> {
    let derived = derive_key(key, document.kdf_salt())?;
    let cipher = XChaCha20Poly1305::new_from_slice(derived.as_ref())
        .map_err(|_| VaultCryptoError::CryptographicFailure)?;
    let aad = document.authenticated_metadata()?;
    let mut plaintext = Zeroizing::new(document.encrypted_payload().to_vec());
    cipher
        .decrypt_in_place_detached(
            XNonce::from_slice(document.nonce()),
            &aad,
            plaintext.as_mut(),
            document.authentication_tag().into(),
        )
        .map_err(|_| VaultCryptoError::CryptographicFailure)?;
    let signer = match plaintext.as_slice() {
        [SECRET_SCHEMA_VERSION, RECOVERY_SEED_KIND, seed @ ..]
            if seed.len() == RECOVERY_SEED_BYTES =>
        {
            let seed: [u8; RECOVERY_SEED_BYTES] = seed
                .try_into()
                .map_err(|_| VaultCryptoError::InvalidSecretPayload)?;
            EvmSigningKey::from_recovery_seed(vault_id, &RecoverySeed::new(seed))?
        }
        [
            SECRET_SCHEMA_VERSION,
            IMPORTED_PRIVATE_KEY_KIND,
            private_key @ ..,
        ] if private_key.len() == PRIVATE_KEY_BYTES => {
            let private_key: [u8; PRIVATE_KEY_BYTES] = private_key
                .try_into()
                .map_err(|_| VaultCryptoError::InvalidSecretPayload)?;
            let imported = ImportedEvmPrivateKey::from_bytes(private_key)
                .map_err(|_| VaultCryptoError::InvalidSecretPayload)?;
            EvmSigningKey::from_imported(vault_id, imported)?
        }
        _ => return Err(VaultCryptoError::InvalidSecretPayload),
    };
    let account = document
        .accounts()
        .first()
        .filter(|_| document.accounts().len() == 1)
        .ok_or(VaultCryptoError::InvalidDocument)?;
    if signer.account().as_str() != account.address() {
        return Err(VaultCryptoError::AccountMismatch);
    }
    Ok(signer)
}

fn seal(
    key: &[u8; 32],
    mut plaintext: Zeroizing<Vec<u8>>,
    accounts: Vec<AccountMetadata>,
) -> Result<VaultDocument, VaultCryptoError> {
    let mut nonce = [0_u8; NONCE_BYTES];
    let mut salt = [0_u8; KDF_SALT_BYTES];
    getrandom::fill(&mut nonce).map_err(|_| VaultCryptoError::RandomnessUnavailable)?;
    getrandom::fill(&mut salt).map_err(|_| VaultCryptoError::RandomnessUnavailable)?;
    let placeholder = VaultDocument::new(
        nonce,
        salt,
        vec![0],
        [0; AUTHENTICATION_TAG_BYTES],
        accounts.clone(),
    )?;
    let aad = placeholder.authenticated_metadata()?;
    let derived = derive_key(key, &salt)?;
    let cipher = XChaCha20Poly1305::new_from_slice(derived.as_ref())
        .map_err(|_| VaultCryptoError::CryptographicFailure)?;
    let tag = cipher
        .encrypt_in_place_detached(XNonce::from_slice(&nonce), &aad, plaintext.as_mut())
        .map_err(|_| VaultCryptoError::CryptographicFailure)?;
    VaultDocument::new(nonce, salt, plaintext.to_vec(), tag.into(), accounts).map_err(Into::into)
}

fn derive_key(
    key: &[u8; 32],
    salt: &[u8; KDF_SALT_BYTES],
) -> Result<Zeroizing<[u8; 32]>, VaultCryptoError> {
    let mut derived = Zeroizing::new([0_u8; 32]);
    Hkdf::<Sha256>::new(Some(salt), key)
        .expand(KDF_INFO.as_bytes(), derived.as_mut())
        .map_err(|_| VaultCryptoError::CryptographicFailure)?;
    Ok(derived)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AccountKeyOrigin, AccountMetadata};

    #[test]
    fn recovery_seed_round_trip_binds_public_account_and_rejects_tampering()
    -> Result<(), Box<dyn Error>> {
        let seed = RecoverySeed::new([7; RECOVERY_SEED_BYTES]);
        let account = crate::derive_default_evm_account(&seed)?;
        let metadata = account.metadata("account-01", 8453)?;
        let key = VaultEncryptionKey::generate()?;
        let document = seal_recovery_seed_vault(&key, &seed, vec![metadata])?;
        let signer =
            open_vault_signing_key(&document, key.expose_secret(), VaultKeyId::new("vault-01")?)?;
        assert_eq!(signer.account(), account.address());

        let changed = VaultDocument::new(
            *document.nonce(),
            *document.kdf_salt(),
            document.encrypted_payload().to_vec(),
            *document.authentication_tag(),
            vec![AccountMetadata::new(
                "account-01",
                "0x0000000000000000000000000000000000000001",
                8453,
                AccountKeyOrigin::recovery_phrase(crate::EVM_DEFAULT_DERIVATION_PATH)?,
            )?],
        )?;
        assert_eq!(
            open_vault_signing_key(&changed, key.expose_secret(), VaultKeyId::new("vault-01")?)
                .err(),
            Some(VaultCryptoError::CryptographicFailure)
        );
        Ok(())
    }
}
