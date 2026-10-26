use super::AppVaultDocument;
use crate::{
    EvmSigningError, EvmSigningKey, EvmSigningObservation, EvmTransactionSignature,
    PasswordVaultError as Error, PasswordVaultJob, VaultEncryptionKey, VaultKeyId,
    VaultKeyStoreError, VaultSession, VaultSigningAuthorization, VaultUnlockAuthorization,
    WalletPassword,
    evm_signing::{SigningAuthorizer, authorize_and_sign},
};
use transaction_policy::{ApprovalCommitment, ApprovalConsumptionStore};
use zeroize::Zeroizing;

impl super::AppVaultOpen {
    /// Activates only one selected entry after full app-password authentication.
    /// No signing authority is issued by this capability.
    /// # Errors
    /// Rejects stale work, wrong session or invalid selected material.
    pub fn activate(
        &self,
        id: &VaultKeyId,
        session: &mut VaultSession,
        now: std::time::Instant,
        job: &PasswordVaultJob,
    ) -> Result<EvmSigningKey, Error> {
        let entry = self.entry(id)?;
        let signer = self.signer(id)?;
        let key =
            VaultEncryptionKey::from_password_bytes(Zeroizing::new(*entry.key.expose_secret()));
        let result = job.with_current(|| {
            session
                .unlock(VaultUnlockAuthorization::new(id.clone(), key), now)
                .map_err(|_| Error::AuthenticationFailed)?;
            Ok(signer)
        });
        if result.is_err() {
            session.lock_manually();
        }
        result
    }
}

/// One-use result of fresh authentication, bound to one root, entry and admitted job.
/// Allows KDF execution on a blocking worker before the short final signing gate.
pub struct AppVaultFreshSigning {
    root: VaultKeyId,
    revision: [u8; 32],
    entry: VaultKeyId,
    signer: EvmSigningKey,
    key: std::sync::Mutex<Option<VaultEncryptionKey>>,
    witness: crate::password_vault::work::JobWitness,
}
impl AppVaultDocument {
    /// Freshly authenticates one signing attempt. A cached open catalogue cannot mint it.
    /// # Errors
    /// Rejects wrong passwords, unknown entries, cancellation and inconsistent accounts.
    pub fn authenticate_signing(
        &self,
        id: &VaultKeyId,
        password: &WalletPassword,
        job: &PasswordVaultJob,
    ) -> Result<AppVaultFreshSigning, Error> {
        let open = self.authenticate(password, job)?;
        let entry = open.entry(id)?;
        Ok(AppVaultFreshSigning {
            root: self.id().clone(),
            revision: self.revision()?,
            entry: id.clone(),
            signer: open.signer(id)?,
            key: std::sync::Mutex::new(Some(VaultEncryptionKey::from_password_bytes(
                Zeroizing::new(*entry.key.expose_secret()),
            ))),
            witness: job.witness(),
        })
    }
}
impl SigningAuthorizer for AppVaultFreshSigning {
    fn authorize(&self, id: &VaultKeyId) -> Result<VaultSigningAuthorization, VaultKeyStoreError> {
        if id != &self.entry {
            return Err(VaultKeyStoreError::AuthorizationDenied);
        }
        let key = self
            .key
            .lock()
            .map_err(|_| VaultKeyStoreError::AuthorizationDenied)?
            .take()
            .ok_or(VaultKeyStoreError::AuthorizationDenied)?;
        Ok(VaultSigningAuthorization::new(id.clone(), key))
    }
}
impl AppVaultFreshSigning {
    /// Consumes fresh authentication through the existing exact approval/signature gate.
    /// # Errors
    /// Rejects a different document/job/session or any final approval observation failure.
    #[expect(
        clippy::too_many_arguments,
        reason = "Root/job identity and the existing exact approval gate remain explicit at this one-use signing boundary"
    )]
    pub fn sign<'binding>(
        self,
        document: &AppVaultDocument,
        job: &PasswordVaultJob,
        commitment: &ApprovalCommitment,
        session: &mut VaultSession,
        store: &impl ApprovalConsumptionStore,
        started: u64,
        observe: impl FnOnce() -> EvmSigningObservation<'binding>,
    ) -> Result<EvmTransactionSignature, EvmSigningError> {
        if self.root != *document.id()
            || document.revision().ok() != Some(self.revision)
            || !self.witness.matches(job)
        {
            return Err(EvmSigningError::AuthorizationCancelled);
        }
        let result = authorize_and_sign(
            &self.signer,
            commitment,
            session,
            &self,
            store,
            started,
            observe,
        )?;
        job.check()
            .map_err(|_| EvmSigningError::AuthorizationCancelled)?;
        Ok(result)
    }
}
