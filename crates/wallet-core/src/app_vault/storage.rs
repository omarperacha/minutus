use super::{AppVaultDocument, document::MAX_BYTES};
use crate::{
    PasswordVaultError as Error, PasswordVaultJob, VaultKeyId,
    password_vault::storage::{EncryptedDocumentStore, StoredPasswordDocument, sealed},
};

impl sealed::Sealed for AppVaultDocument {}
impl StoredPasswordDocument for AppVaultDocument {
    const MAX_BYTES: usize = MAX_BYTES;
    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        Self::from_json(bytes)
    }
    fn encode(&self) -> Result<Vec<u8>, Error> {
        self.to_json()
    }
    fn identity(&self) -> &VaultKeyId {
        self.id()
    }
}

/// Single aggregate root in a separate private application-vault directory.
pub struct AppVaultStore(EncryptedDocumentStore<AppVaultDocument>);
impl AppVaultStore {
    /// Pins private storage; does not open any old wallet namespace or Keychain item.
    /// # Errors
    /// Rejects unsafe storage, multiple roots and malformed encrypted documents.
    pub fn initialize(path: &std::path::Path) -> Result<Self, Error> {
        let store = Self(EncryptedDocumentStore::initialize(path)?);
        Ok(store)
    }
    /// Loads the sole root, preserving malformed/unavailable state as errors.
    /// # Errors
    /// Rejects multiple roots and malformed/unsafe storage.
    pub fn load(&self) -> Result<Option<AppVaultDocument>, Error> {
        let ids = self.0.vault_ids()?;
        if ids.len() > 1 {
            return Err(Error::Conflict);
        }
        ids.first().map(|id| self.0.load(id)).transpose()
    }
    /// Creates only in an empty aggregate namespace.
    /// # Errors
    /// Rejects competing creation, cancellation and storage errors.
    pub fn create(&self, document: &AppVaultDocument, job: &PasswordVaultJob) -> Result<(), Error> {
        self.0.create_in_empty_store(document, job)
    }
    /// Atomically commits one exact root revision, including catalogue mutations.
    /// Callers retain the authenticated/confirmed native mutation capability.
    /// # Errors
    /// Rejects changed identity/revision, cancellation or uncertain publication.
    pub fn replace(
        &self,
        old: &AppVaultDocument,
        changed: &AppVaultDocument,
        job: &PasswordVaultJob,
    ) -> Result<(), Error> {
        self.0.replace_record(old, changed, job)
    }
    /// Holds exact revision ownership across a bounded non-interactive operation.
    /// # Errors
    /// Rejects concurrent changes and operation failures.
    pub fn with_revision<T>(
        &self,
        expected: &AppVaultDocument,
        effect: impl FnOnce() -> Result<T, Error>,
    ) -> Result<T, Error> {
        self.0.with_revision(expected, effect)
    }
    /// Removes only the exact confirmed/authenticated aggregate revision.
    /// # Errors
    /// Rejects changed state, cancellation or incomplete durability.
    pub(crate) fn remove(
        &self,
        expected: &AppVaultDocument,
        job: &PasswordVaultJob,
    ) -> Result<(), Error> {
        self.0.remove_record(expected, job)
    }
}

impl AppVaultStore {
    /// Deletes after fresh password authentication and exact native confirmation.
    /// # Errors
    /// Rejects wrong password, wrong target or concurrent replacement.
    pub fn delete(
        &self,
        expected: &AppVaultDocument,
        password: &crate::WalletPassword,
        commit: crate::VaultDeletionCommit,
        job: &PasswordVaultJob,
    ) -> Result<(), Error> {
        drop(expected.authenticate(password, job)?);
        let timeout = crate::VaultInactivityTimeout::new(std::time::Duration::from_mins(5))
            .map_err(|_| Error::InvalidDocument)?;
        let mut session = crate::VaultSession::new(expected.id().clone(), timeout);
        let id = commit
            .authorize_password_removal(&mut session)
            .map_err(|_| Error::AuthenticationFailed)?;
        if &id != expected.id() {
            return Err(Error::Conflict);
        }
        self.remove(expected, job)
    }
    /// Removes the last entry only from a freshly authenticated, now empty catalogue.
    /// # Errors
    /// Rejects unrelated/revised roots and catalogues that still contain accounts.
    pub fn remove_empty(
        &self,
        expected: &AppVaultDocument,
        opened: super::AppVaultOpen,
        job: &PasswordVaultJob,
    ) -> Result<(), Error> {
        if !opened.matches(expected)? || !opened.entries().is_empty() {
            return Err(Error::Conflict);
        }
        drop(opened);
        self.remove(expected, job)
    }
}

/// Distinct one-use destructive recovery-reset confirmation; no old-key authority.
pub struct AppVaultResetCommit {
    id: VaultKeyId,
    revision: [u8; 32],
}
impl AppVaultResetCommit {
    /// Binds explicit whole-vault loss acknowledgement and exact public confirmation.
    /// # Errors
    /// Rejects missing acknowledgement or a different confirmation line.
    pub fn confirm(
        document: &AppVaultDocument,
        all_other_accounts_may_be_lost: bool,
        typed: crate::VaultTypedConfirmation,
    ) -> Result<Self, Error> {
        if !all_other_accounts_may_be_lost
            || typed.into_value().as_str() != format!("RESET {}", document.id().as_str())
        {
            return Err(Error::AuthenticationFailed);
        }
        Ok(Self {
            id: document.id().clone(),
            revision: document.revision()?,
        })
    }
}
impl AppVaultStore {
    /// Replaces inaccessible v3 only after explicit native recovery reset confirmation.
    /// The replacement is built from independently validated recovery material.
    /// # Errors
    /// Rejects wrong targets, stale revisions, cancellation or uncertain publication.
    pub fn reset(
        &self,
        previous: &AppVaultDocument,
        replacement: &AppVaultDocument,
        commit: AppVaultResetCommit,
        job: &PasswordVaultJob,
    ) -> Result<(), Error> {
        let AppVaultResetCommit { id, revision } = commit;
        if id != *previous.id() || revision != previous.revision()? {
            return Err(Error::Conflict);
        }
        self.replace(previous, replacement, job)
    }
}
