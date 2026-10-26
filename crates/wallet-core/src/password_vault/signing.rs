use super::{PasswordVaultDocument, PasswordVaultError, PasswordVaultJob, WalletPassword};
use crate::{
    EvmSigningError, EvmSigningKey, EvmSigningObservation, EvmTransactionSignature, VaultKeyId,
    VaultKeyStoreError, VaultSession, VaultSigningAuthorization,
    evm_signing::{SigningAuthorizer, authorize_and_sign},
};
use transaction_policy::{
    ApprovalCommitment, ApprovalConsumptionStore, ApprovalInvalidationReason,
};

/// Borrowed one-attempt native credential binding. No raw-digest signing API.
pub struct PasswordSigningAttempt<'a> {
    document: &'a PasswordVaultDocument,
    password: &'a WalletPassword,
    job: &'a PasswordVaultJob,
}

impl PasswordVaultDocument {
    /// Binds the exact encrypted document, fresh native password and current job
    /// to the existing approval gate. An unlocked DEK cannot construct this input.
    #[must_use]
    pub const fn signing_attempt<'a>(
        &'a self,
        password: &'a WalletPassword,
        job: &'a PasswordVaultJob,
    ) -> PasswordSigningAttempt<'a> {
        PasswordSigningAttempt {
            document: self,
            password,
            job,
        }
    }
}

impl SigningAuthorizer for PasswordSigningAttempt<'_> {
    fn authorize(
        &self,
        key_id: &VaultKeyId,
    ) -> Result<VaultSigningAuthorization, VaultKeyStoreError> {
        if self.document.id() != key_id {
            return Err(VaultKeyStoreError::AuthorizationDenied);
        }
        let (key, _validated_signer) = self
            .document
            .authenticate(self.password, self.job)
            .map_err(|error| {
                if error == PasswordVaultError::Cancelled {
                    VaultKeyStoreError::AuthorizationCancelled
                } else {
                    VaultKeyStoreError::AuthorizationDenied
                }
            })?;
        Ok(VaultSigningAuthorization::new(key_id.clone(), key))
    }
}

impl PasswordSigningAttempt<'_> {
    /// Uses the existing session → fresh authentication → fresh observation →
    /// exact approval verification → durable consumption → signing sequence.
    ///
    /// # Errors
    /// Any authentication, lifecycle, account, commitment, durable-store or
    /// signature failure releases no signature. Interrupted attempts never retry.
    pub fn sign<'binding>(
        &self,
        key: &EvmSigningKey,
        commitment: &ApprovalCommitment,
        session: &mut VaultSession,
        store: &impl ApprovalConsumptionStore,
        started_at_unix: u64,
        observe: impl FnOnce() -> EvmSigningObservation<'binding>,
    ) -> Result<EvmTransactionSignature, EvmSigningError> {
        if self.document.payload().accounts().len() != 1
            || self.document.payload().accounts()[0].address() != key.account().as_str()
        {
            commitment.invalidate(ApprovalInvalidationReason::Failed, started_at_unix, store)?;
            return Err(EvmSigningError::ApprovalForDifferentAccount);
        }
        let signature = authorize_and_sign(
            key,
            commitment,
            session,
            self,
            store,
            started_at_unix,
            observe,
        )?;
        self.job
            .check()
            .map_err(|_| EvmSigningError::AuthorizationCancelled)?;
        Ok(signature)
    }
}
