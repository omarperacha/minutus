use super::{PasswordVaultDocument, PasswordVaultError, PasswordVaultJob, WalletPassword, kdf};
use crate::{
    AccountMetadata, EvmSigningKey, RecoverySeed, VaultEncryptionKey, VaultKeyId, VaultSession,
    VaultUnlockAuthorization, open_vault_signing_key, seal_recovery_seed_vault,
};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{AeadInPlace as _, KeyInit as _},
};
use std::time::Instant;
use zeroize::Zeroizing;

impl PasswordVaultDocument {
    /// Seals a verified recovery seed using the existing payload crypto and a fresh
    /// password-wrapped random DEK. No native activation or persistence is implied.
    ///
    /// # Errors
    /// Returns cancellation, account/format, bounded KDF or cryptographic errors.
    pub fn create_recovery(
        id: VaultKeyId,
        password: &WalletPassword,
        seed: &RecoverySeed,
        accounts: Vec<AccountMetadata>,
        job: &PasswordVaultJob,
    ) -> Result<Self, PasswordVaultError> {
        job.check()?;
        if accounts.len() != 1 {
            return Err(PasswordVaultError::InvalidDocument);
        }
        let key = VaultEncryptionKey::generate()
            .map_err(|_| PasswordVaultError::RandomnessUnavailable)?;
        let payload = seal_recovery_seed_vault(&key, seed, accounts)
            .map_err(|_| PasswordVaultError::CryptographyUnavailable)?;
        open_vault_signing_key(&payload, key.expose_secret(), id.clone())
            .map_err(|_| PasswordVaultError::AuthenticationFailed)?;
        let mut document = Self {
            id,
            salt: [0; 32],
            nonce: [0; 24],
            encrypted_key: [0; 32],
            tag: [0; 16],
            payload,
        };
        document.wrap(password, &key, job)?;
        Ok(document)
    }

    /// Authenticates both the wrapper and payload into a one-use unlock capability.
    ///
    /// # Errors
    /// Wrong password/corruption, stale work and resource failure fail closed.
    pub fn authenticate_unlock<'job>(
        &self,
        password: &WalletPassword,
        job: &'job PasswordVaultJob,
    ) -> Result<PasswordVaultUnlock<'job>, PasswordVaultError> {
        let (key, signer) = self.authenticate(password, job)?;
        Ok(PasswordVaultUnlock {
            job,
            id: self.id.clone(),
            key,
            signer,
        })
    }

    /// Rewraps the identical DEK/payload after fresh current-password authentication.
    /// Verifies the replacement before it can be published; old copies remain old
    /// password routes. This does not rotate an on-chain key.
    ///
    /// # Errors
    /// Returns authentication, cancellation or bounded cryptographic failures.
    pub fn change_password(
        &self,
        current: &WalletPassword,
        replacement: &WalletPassword,
        job: &PasswordVaultJob,
    ) -> Result<Self, PasswordVaultError> {
        let (key, _signer) = self.authenticate(current, job)?;
        let mut changed = self.clone();
        changed.wrap(replacement, &key, job)?;
        let (_verified_key, _verified_signer) = changed.authenticate(replacement, job)?;
        job.check()?;
        Ok(changed)
    }

    pub(super) fn authenticate(
        &self,
        password: &WalletPassword,
        job: &PasswordVaultJob,
    ) -> Result<(VaultEncryptionKey, EvmSigningKey), PasswordVaultError> {
        job.check()?;
        let derived = kdf::derive(password, &self.salt)?;
        job.check()?;
        let cipher = XChaCha20Poly1305::new_from_slice(derived.as_ref())
            .map_err(|_| PasswordVaultError::CryptographyUnavailable)?;
        let mut key = Zeroizing::new(self.encrypted_key);
        cipher
            .decrypt_in_place_detached(
                XNonce::from_slice(&self.nonce),
                &self.aad()?,
                key.as_mut(),
                (&self.tag).into(),
            )
            .map_err(|_| {
                job.authentication_failed();
                PasswordVaultError::AuthenticationFailed
            })?;
        let signer =
            open_vault_signing_key(&self.payload, &key, self.id.clone()).map_err(|_| {
                job.authentication_failed();
                PasswordVaultError::AuthenticationFailed
            })?;
        job.check()?;
        job.authentication_succeeded();
        Ok((VaultEncryptionKey::from_password_bytes(key), signer))
    }

    fn wrap(
        &mut self,
        password: &WalletPassword,
        key: &VaultEncryptionKey,
        job: &PasswordVaultJob,
    ) -> Result<(), PasswordVaultError> {
        getrandom::fill(&mut self.salt).map_err(|_| PasswordVaultError::RandomnessUnavailable)?;
        getrandom::fill(&mut self.nonce).map_err(|_| PasswordVaultError::RandomnessUnavailable)?;
        job.check()?;
        let derived = kdf::derive(password, &self.salt)?;
        job.check()?;
        let cipher = XChaCha20Poly1305::new_from_slice(derived.as_ref())
            .map_err(|_| PasswordVaultError::CryptographyUnavailable)?;
        let mut plaintext = Zeroizing::new(*key.expose_secret());
        self.tag = cipher
            .encrypt_in_place_detached(
                XNonce::from_slice(&self.nonce),
                &self.aad()?,
                plaintext.as_mut(),
            )
            .map_err(|_| PasswordVaultError::CryptographyUnavailable)?
            .into();
        self.encrypted_key = *plaintext;
        job.check()
    }
}

/// One-use native unlock result containing validated secret owners; never IPC-safe.
pub struct PasswordVaultUnlock<'job> {
    job: &'job PasswordVaultJob,
    id: VaultKeyId,
    key: VaultEncryptionKey,
    signer: EvmSigningKey,
}
impl PasswordVaultUnlock<'_> {
    /// Installs only into the matching live session under the job's cancellation gate.
    ///
    /// # Errors
    /// Rejects stale work, a different vault or an already unlocked session.
    pub fn install(
        self,
        session: &mut VaultSession,
        now: Instant,
        job: &PasswordVaultJob,
    ) -> Result<EvmSigningKey, PasswordVaultError> {
        if !std::ptr::eq(self.job, job) {
            return Err(PasswordVaultError::Cancelled);
        }
        let result = job.with_current(|| {
            if session.key_id() != &self.id {
                return Err(PasswordVaultError::AuthenticationFailed);
            }
            session
                .unlock(VaultUnlockAuthorization::new(self.id, self.key), now)
                .map_err(|_| PasswordVaultError::AuthenticationFailed)?;
            Ok(self.signer)
        });
        if result.is_err() {
            session.lock_manually();
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AccountKeyOrigin, EvmPrivateKeyImportPreview, EvmPrivateKeyInput, PasswordVaultWork,
    };

    #[test]
    fn password_change_preserves_existing_imported_payload_identity_without_exposing_import_ui()
    -> Result<(), Box<dyn std::error::Error>> {
        // Compatibility fixture only; the native experiment creates seed wallets.
        let input = EvmPrivateKeyInput::new("42".repeat(32))?;
        let preview = EvmPrivateKeyImportPreview::validate(input)?;
        let address = preview.address().as_str().to_owned();
        let acknowledgement = preview.acknowledge_recovery_warning(true)?;
        let commit = preview.confirm(&acknowledgement)?;
        let key = VaultEncryptionKey::generate()?;
        let payload = crate::seal_imported_private_key_vault(
            &key,
            commit,
            vec![AccountMetadata::new(
                "imported-1",
                &address,
                84532,
                AccountKeyOrigin::ImportedPrivateKey,
            )?],
        )?;
        let mut document = PasswordVaultDocument {
            id: VaultKeyId::new("synthetic-imported")?,
            salt: [0; 32],
            nonce: [0; 24],
            encrypted_key: [0; 32],
            tag: [0; 16],
            payload,
        };
        let password = WalletPassword::new("synthetic imported password".to_owned())?;
        let replacement = WalletPassword::new("synthetic imported replacement".to_owned())?;
        let work = PasswordVaultWork::new();
        let job = work.begin()?;
        document.wrap(&password, &key, &job)?;
        let changed = document.change_password(&password, &replacement, &job)?;
        assert_eq!(changed.payload(), document.payload());
        let (_, signer) = changed.authenticate(&replacement, &job)?;
        assert_eq!(signer.account().as_str(), address);
        Ok(())
    }
}
