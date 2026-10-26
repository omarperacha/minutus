use super::synthetic::SyntheticCandidate;
use crate::wallet_lifecycle::native::application_is_active_after_authorization;
use app_core::AppCore;
use std::{
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use transaction_policy::ApprovalCommitment;
use wallet_core::{
    ConfirmedWalletRestore, EvmSigningKey, EvmSigningObservation, PasswordVaultDocument,
    PasswordVaultError as Error, PasswordVaultJob, PasswordVaultStore, PasswordVaultWork,
    RecoverySeed, VaultDeletionCommit, VaultInactivityTimeout, VaultInventory, VaultKeyId,
    VaultSession, VaultSessionStatus, WalletPassword, WalletRestoreChoice,
};

struct Selected {
    document: PasswordVaultDocument,
    session: VaultSession,
    signer: Option<EvmSigningKey>,
}
impl Selected {
    fn locked(document: PasswordVaultDocument) -> Result<Self, Error> {
        let timeout = VaultInactivityTimeout::new(Duration::from_mins(5))
            .map_err(|_| Error::InvalidDocument)?;
        Ok(Self {
            session: VaultSession::new(document.id().clone(), timeout),
            document,
            signer: None,
        })
    }
    fn lock(&mut self) {
        self.signer = None;
        self.session.lock_manually();
    }
}

pub(super) struct Control {
    store: PasswordVaultStore,
    selected: Mutex<Option<Selected>>,
    pub(super) work: PasswordVaultWork,
    lock_pending: AtomicBool,
    lock_queued: AtomicBool,
}
impl Control {
    pub(super) fn initialize(directory: &Path) -> Result<Self, Error> {
        let store = PasswordVaultStore::initialize(directory)?;
        let ids = store.vault_ids()?;
        if ids.len() > 1 {
            return Err(Error::Conflict);
        }
        let selected = ids
            .first()
            .map(|id| store.load(id).and_then(Selected::locked))
            .transpose()?;
        Ok(Self {
            store,
            selected: Mutex::new(selected),
            work: PasswordVaultWork::new(),
            lock_pending: AtomicBool::new(false),
            lock_queued: AtomicBool::new(false),
        })
    }
    pub(super) fn lock(&self) {
        self.work.cancel();
        self.lock_pending.store(true, Ordering::Release);
        if let Ok(mut selected) = self.selected.try_lock() {
            if let Some(wallet) = selected.as_mut() {
                wallet.lock();
            }
            self.lock_pending.store(false, Ordering::Release);
        }
    }
    pub(super) fn begin_operation(&self) -> Result<PasswordVaultJob, Error> {
        if self.lock_queued.load(Ordering::Acquire) {
            return Err(Error::Busy);
        }
        self.work.begin()
    }
    pub(super) fn should_lock_for(event: desktop_lifecycle::LifecycleEvent) -> bool {
        // Match the ordinary wallet/x402 lifecycle: changing the frontmost app
        // is not a session lock. Signing independently re-observes foreground
        // after fresh authentication and rechecks exact approval/session freshness.
        event == desktop_lifecycle::LifecycleEvent::SecurityBoundary
    }
    pub(super) fn queue_lock(&self) -> bool {
        // Revoke host-visible readiness before waiting for any filesystem commit
        // guard. Native notification callbacks must never wait on fsync or a KDF.
        self.lock_pending.store(true, Ordering::Release);
        !self.lock_queued.swap(true, Ordering::AcqRel)
    }
    pub(super) fn complete_queued_lock(&self) {
        self.lock();
        self.lock_queued.store(false, Ordering::Release);
    }
    pub(super) fn enforce_inactivity(&self, now: Instant) {
        if let Ok(mut selected) = self.selected.try_lock()
            && let Some(wallet) = selected.as_mut()
            && !matches!(
                wallet.session.enforce_inactivity(now),
                VaultSessionStatus::Unlocked
            )
        {
            wallet.signer = None;
        }
    }
    pub(super) fn summary(&self) -> String {
        let Ok(selected) = self.selected.try_lock() else {
            return "Test wallet operation in progress".to_owned();
        };
        selected.as_ref().map_or_else(
            || "No test wallet is configured".to_owned(),
            |wallet| {
                format!(
                    "Unfunded Base Sepolia test wallet\n{}\n{}",
                    wallet.document.payload().accounts()[0].address(),
                    if wallet.signer.is_some() && !self.lock_pending.load(Ordering::Acquire) {
                        "Unlocked"
                    } else {
                        "Locked"
                    }
                )
            },
        )
    }
    pub(super) fn id(&self) -> Result<VaultKeyId, Error> {
        self.selected
            .lock()
            .map_err(|_| Error::Cancelled)?
            .as_ref()
            .map(|s| s.document.id().clone())
            .ok_or(Error::NotFound)
    }
    pub(super) fn account(&self) -> Result<String, Error> {
        let selected = self.selected.lock().map_err(|_| Error::Cancelled)?;
        let wallet = selected.as_ref().ok_or(Error::NotFound)?;
        if wallet.signer.is_none()
            || self.lock_pending.load(Ordering::Acquire)
            || self.lock_queued.load(Ordering::Acquire)
        {
            return Err(Error::AuthenticationFailed);
        }
        Ok(wallet.document.payload().accounts()[0].address().to_owned())
    }
    pub(super) fn create(
        &self,
        id: VaultKeyId,
        password: &WalletPassword,
        seed: &RecoverySeed,
        job: &PasswordVaultJob,
    ) -> Result<(), Error> {
        let mut selected = self.selected.lock().map_err(|_| Error::Cancelled)?;
        self.lock_pending.store(false, Ordering::Release);
        job.check()?;
        if selected.is_some() || !self.store.vault_ids()?.is_empty() {
            return Err(Error::Conflict);
        }
        self.install_new_locked(&mut selected, id, password, seed, job)
    }

    pub(super) fn recovery_inventory(&self) -> Result<VaultInventory, Error> {
        let selected = self.selected.lock().map_err(|_| Error::Cancelled)?;
        self.empty_inventory(selected.as_ref())
    }

    pub(super) fn restore(
        &self,
        confirmed: ConfirmedWalletRestore,
        password: &WalletPassword,
        job: &PasswordVaultJob,
    ) -> Result<(), Error> {
        let mut selected = self.selected.lock().map_err(|_| Error::Cancelled)?;
        self.lock_pending.store(false, Ordering::Release);
        job.check()?;
        let current = self.empty_inventory(selected.as_ref())?;
        let commit = confirmed
            .authorize_commit(&current)
            .map_err(|_| Error::Conflict)?;
        let WalletRestoreChoice::Add { new_vault_id } = commit.choice() else {
            return Err(Error::Conflict);
        };
        let id = new_vault_id.clone();
        let seed = commit.into_seed();
        self.install_new_locked(&mut selected, id, password, &seed, job)
    }

    fn empty_inventory(&self, selected: Option<&Selected>) -> Result<VaultInventory, Error> {
        let ids = self.store.vault_ids()?;
        if selected.is_some() || !ids.is_empty() {
            return Err(Error::Conflict);
        }
        Ok(VaultInventory::empty())
    }

    fn install_new_locked(
        &self,
        selected: &mut Option<Selected>,
        id: VaultKeyId,
        password: &WalletPassword,
        seed: &RecoverySeed,
        job: &PasswordVaultJob,
    ) -> Result<(), Error> {
        let account = wallet_core::derive_default_evm_account(seed)
            .map_err(|_| Error::CryptographyUnavailable)?;
        let metadata = account
            .metadata("account-1", 84532)
            .map_err(|_| Error::InvalidDocument)?;
        let document =
            PasswordVaultDocument::create_recovery(id, password, seed, vec![metadata], job)?;
        // Verify the complete wrapper before persistence. Successful onboarding
        // leaves a saved, locked wallet; window changes cannot activate a session.
        drop(document.authenticate_unlock(password, job)?);
        self.store.create_in_empty_store(&document, job)?;
        *selected = Some(Selected::locked(document.clone())?);
        let wallet = selected.as_mut().ok_or(Error::NotFound)?;
        self.finish(wallet, job, Ok(()))
    }
    pub(super) fn unlock(
        &self,
        password: &WalletPassword,
        job: &PasswordVaultJob,
    ) -> Result<(), Error> {
        let mut selected = self.selected.lock().map_err(|_| Error::Cancelled)?;
        self.lock_pending.store(false, Ordering::Release);
        job.check()?;
        let wallet = selected.as_mut().ok_or(Error::NotFound)?;
        wallet.lock();
        let document = self.current(wallet)?;
        let result = document
            .authenticate_unlock(password, job)
            .and_then(|unlock| {
                self.store.with_revision(&document, || {
                    unlock.install(&mut wallet.session, Instant::now(), job)
                })
            });
        match result {
            Ok(signer) => wallet.signer = Some(signer),
            Err(error) => {
                wallet.lock();
                return Err(error);
            }
        }
        self.finish(wallet, job, Ok(()))
    }
    pub(super) fn verify_password_for_change(
        &self,
        current: &WalletPassword,
        job: &PasswordVaultJob,
    ) -> Result<(), Error> {
        let mut selected = self.selected.lock().map_err(|_| Error::Cancelled)?;
        self.lock_pending.store(false, Ordering::Release);
        job.check()?;
        let wallet = selected.as_mut().ok_or(Error::NotFound)?;
        wallet.lock();
        let document = self.current(wallet)?;
        // Validate before new-password entry, then discard all decrypted state.
        // The eventual commit rechecks the revision and authenticates again.
        let result = document
            .authenticate_unlock(current, job)
            .and_then(|verified| {
                drop(verified);
                self.store.with_revision(&document, || job.check())
            });
        self.finish(wallet, job, result)
    }

    pub(super) fn change_password(
        &self,
        current: &WalletPassword,
        replacement: &WalletPassword,
        job: &PasswordVaultJob,
    ) -> Result<(), Error> {
        let mut selected = self.selected.lock().map_err(|_| Error::Cancelled)?;
        self.lock_pending.store(false, Ordering::Release);
        job.check()?;
        let wallet = selected.as_mut().ok_or(Error::NotFound)?;
        wallet.lock();
        let previous = self.current(wallet)?;
        let changed = previous.change_password(current, replacement, job)?;
        let result = self.store.replace(&previous, &changed, job);
        if result.is_ok() {
            wallet.document = changed;
        } else if result == Err(Error::CommitUncertain) {
            wallet.document = self.store.load(previous.id())?;
        }
        self.finish(wallet, job, result)
    }
    pub(super) fn sign(
        &self,
        core: &AppCore,
        candidate: &SyntheticCandidate,
        approval: &ApprovalCommitment,
        password: &WalletPassword,
        job: &PasswordVaultJob,
    ) -> Result<(), Error> {
        let mut selected = self.selected.lock().map_err(|_| Error::Cancelled)?;
        self.lock_pending.store(false, Ordering::Release);
        job.check()?;
        let wallet = selected.as_mut().ok_or(Error::NotFound)?;
        let document = self.current(wallet)?;
        let key = wallet.signer.as_ref().ok_or(Error::AuthenticationFailed)?;
        let result = self.store.with_revision(&document, || {
            document
                .signing_attempt(password, job)
                .sign(
                    key,
                    approval,
                    &mut wallet.session,
                    core,
                    candidate.now,
                    || {
                        EvmSigningObservation::new(
                            candidate.binding(),
                            super::synthetic::unix_now(),
                            Instant::now(),
                            application_is_active_after_authorization(),
                        )
                    },
                )
                .map_err(|_| Error::AuthenticationFailed)
                .and_then(|signature| {
                    signature
                        .verify(&candidate.digest, candidate.account.as_str())
                        .map_err(|_| Error::AuthenticationFailed)
                })
        });
        self.finish(wallet, job, result)
    }
    pub(super) fn delete(
        &self,
        password: &WalletPassword,
        commit: VaultDeletionCommit,
        job: &PasswordVaultJob,
    ) -> Result<(), Error> {
        let mut selected = self.selected.lock().map_err(|_| Error::Cancelled)?;
        self.lock_pending.store(false, Ordering::Release);
        job.check()?;
        let wallet = selected.as_mut().ok_or(Error::NotFound)?;
        wallet.lock();
        let document = self.current(wallet)?;
        let _authenticated = document.authenticate_unlock(password, job)?;
        self.store
            .delete(&document, commit, &mut wallet.session, job)?;
        *selected = None;
        Ok(())
    }
    fn current(&self, wallet: &mut Selected) -> Result<PasswordVaultDocument, Error> {
        let current = match self.store.load(wallet.document.id()) {
            Ok(document) => document,
            Err(error) => {
                wallet.lock();
                return Err(error);
            }
        };
        if current.revision()? != wallet.document.revision()? {
            wallet.lock();
            wallet.document = current;
            return Err(Error::Conflict);
        }
        Ok(current)
    }
    fn finish(
        &self,
        wallet: &mut Selected,
        job: &PasswordVaultJob,
        result: Result<(), Error>,
    ) -> Result<(), Error> {
        let current = job.check();
        let pending = self.lock_pending.swap(false, Ordering::AcqRel)
            || self.lock_queued.load(Ordering::Acquire);
        if result.is_err() || current.is_err() || pending {
            wallet.lock();
        }
        if pending {
            return Err(Error::Cancelled);
        }
        current?;
        result
    }
}
