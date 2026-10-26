//! Storage failure stays local to the wallet; it cannot become an empty vault.
use wallet_core::{
    AppVaultDocument, AppVaultOpen, AppVaultResetCommit, AppVaultStore,
    PasswordVaultError as Error, PasswordVaultJob, VaultDeletionCommit, WalletPassword,
};
pub(super) struct WalletStorage(Result<AppVaultStore, Error>);
impl WalletStorage {
    pub(super) fn initialize(path: &std::path::Path) -> Self {
        Self(AppVaultStore::initialize(path))
    }
    fn store(&self) -> Result<&AppVaultStore, Error> {
        self.0.as_ref().map_err(|e| *e)
    }
    pub(super) fn load(&self) -> Result<Option<AppVaultDocument>, Error> {
        self.store()?.load()
    }
    pub(super) fn create(
        &self,
        doc: &AppVaultDocument,
        job: &PasswordVaultJob,
    ) -> Result<(), Error> {
        self.store()?.create(doc, job)
    }
    pub(super) fn replace(
        &self,
        old: &AppVaultDocument,
        new: &AppVaultDocument,
        job: &PasswordVaultJob,
    ) -> Result<(), Error> {
        self.store()?.replace(old, new, job)
    }
    pub(super) fn with_revision<T>(
        &self,
        doc: &AppVaultDocument,
        effect: impl FnOnce() -> Result<T, Error>,
    ) -> Result<T, Error> {
        self.store()?.with_revision(doc, effect)
    }
    pub(super) fn delete(
        &self,
        doc: &AppVaultDocument,
        password: &WalletPassword,
        commit: VaultDeletionCommit,
        job: &PasswordVaultJob,
    ) -> Result<(), Error> {
        self.store()?.delete(doc, password, commit, job)
    }
    pub(super) fn remove_empty(
        &self,
        doc: &AppVaultDocument,
        open: AppVaultOpen,
        job: &PasswordVaultJob,
    ) -> Result<(), Error> {
        self.store()?.remove_empty(doc, open, job)
    }
    pub(super) fn reset(
        &self,
        old: &AppVaultDocument,
        new: &AppVaultDocument,
        commit: AppVaultResetCommit,
        job: &PasswordVaultJob,
    ) -> Result<(), Error> {
        self.store()?.reset(old, new, commit, job)
    }
}
