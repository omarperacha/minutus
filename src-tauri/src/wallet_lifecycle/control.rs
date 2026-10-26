//! Sole normal-wallet owner: shared app session, selected account and root revision.
use super::{password_error, wallet_error};
use ipc_types::{IpcError, PublicWalletState, WalletAvailability, WalletId};
use std::{
    ops::Deref,
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use wallet_core::{
    AppVaultDocument, AppVaultOpen, PasswordVaultError as Error, PasswordVaultJob,
    PasswordVaultWork, VaultInactivityTimeout, VaultKeyId, VaultSession, WalletPassword,
};

pub(super) struct LiveVault {
    pub(super) document: Option<AppVaultDocument>,
    pub(super) selected: Option<VaultKeyId>,
    pub(super) opened: Option<AppVaultOpen>,
    pub(super) session: Option<VaultSession>,
    pub(super) last_activity: Option<Instant>,
}
impl LiveVault {
    pub(super) fn lock(&mut self) {
        self.opened = None;
        self.last_activity = None;
        if let Some(session) = &mut self.session {
            session.lock_manually();
        }
    }
    fn enforce(&mut self, now: Instant) {
        if self.last_activity.is_some_and(|last| {
            now.checked_duration_since(last)
                .is_none_or(|elapsed| elapsed >= Duration::from_mins(5))
        }) {
            self.lock();
        }
    }
}
pub(crate) struct Control {
    pub(super) store: super::storage::WalletStorage,
    pub(super) state: Mutex<LiveVault>,
    pub(super) work: PasswordVaultWork,
    pub(super) revoked: AtomicBool,
    pub(super) epoch: AtomicU64,
    cleanup_pending: AtomicBool,
    pub(super) app: OnceLock<tauri::AppHandle>,
    pub(super) old_format_present: bool,
    pub(super) missing_vault: AtomicBool,
}
/// Cloneable composition handle; all clones share one privileged owner.
#[derive(Clone)]
pub(crate) struct WalletControl(Arc<Control>);
impl Deref for WalletControl {
    type Target = Control;
    fn deref(&self) -> &Control {
        &self.0
    }
}
impl WalletControl {
    pub(crate) fn initialize(
        directory: impl Into<PathBuf>,
        selected: Option<&str>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let previous_selection = selected.is_some_and(|id| id.starts_with("account-"));
        let directory = directory.into();
        let store = super::storage::WalletStorage::initialize(&directory);
        // An unreadable vault is unavailable, never a reason to stop inference/chat startup.
        // Every wallet operation still loads the store and fails closed.
        let document = store.load().unwrap_or(None);
        let missing_vault = document.is_none() && previous_selection;
        let inventory = document
            .as_ref()
            .map(AppVaultDocument::inventory)
            .transpose()?
            .unwrap_or_default();
        let selected = inventory
            .iter()
            .find(|(id, _)| Some(id.as_str()) == selected)
            .or_else(|| inventory.first())
            .map(|(id, _)| id.clone());
        let session = selected.clone().map(locked_session).transpose()?;
        // Name-only incompatibility detection. Never open old wallet files or probe Keychain.
        let old_format_present = directory
            .parent()
            .map(|p| p.join("wallets"))
            .filter(|p| p.is_dir())
            .map(|p| std::fs::read_dir(p).map(|mut entries| entries.next().is_some()))
            .transpose()?
            .unwrap_or(false);
        Ok(Self(Arc::new(Control {
            store,
            state: Mutex::new(LiveVault {
                document,
                selected,
                session,
                opened: None,
                last_activity: None,
            }),
            work: PasswordVaultWork::new(),
            revoked: AtomicBool::new(false),
            epoch: AtomicU64::new(0),
            cleanup_pending: AtomicBool::new(false),
            app: OnceLock::new(),
            old_format_present,
            missing_vault: AtomicBool::new(missing_vault),
        })))
    }
    pub(crate) fn bind_app(&self, app: &tauri::AppHandle) {
        let _ = self.app.set(app.clone());
    }
    pub(super) fn app(&self) -> Result<&tauri::AppHandle, IpcError> {
        self.app.get().ok_or_else(IpcError::internal)
    }
    pub(super) fn begin(&self) -> Result<PasswordVaultJob, IpcError> {
        if cfg!(all(feature = "password-vault-prototype", not(test))) {
            return Err(wallet_error(
                "Normal wallet actions are disabled in the isolated prototype build",
                false,
            ));
        }
        if self.cleanup_pending.load(Ordering::Acquire) {
            return Err(password_error(Error::Busy));
        }
        let job = self.work.begin().map_err(password_error)?;
        let mut state = self.state.lock().map_err(|_| IpcError::internal())?;
        if self.revoked.swap(false, Ordering::AcqRel) {
            state.lock();
        }
        job.check().map_err(password_error)?;
        Ok(job)
    }
    pub(super) fn snapshot(
        &self,
    ) -> Result<(Option<AppVaultDocument>, Option<VaultKeyId>), IpcError> {
        let current = self.store.load().map_err(password_error)?;
        let mut state = self.state.lock().map_err(|_| IpcError::internal())?;
        if current != state.document {
            if current.is_none() && state.document.is_some() {
                self.missing_vault.store(true, Ordering::Release);
            }
            state.lock();
            self.epoch.fetch_add(1, Ordering::AcqRel);
            state.document = current;
            let inventory = state
                .document
                .as_ref()
                .map(AppVaultDocument::inventory)
                .transpose()
                .map_err(password_error)?
                .unwrap_or_default();
            if !inventory
                .iter()
                .any(|(id, _)| Some(id) == state.selected.as_ref())
            {
                state.selected = inventory.first().map(|(id, _)| id.clone());
            }
            state.session = state
                .selected
                .clone()
                .map(locked_session)
                .transpose()
                .map_err(password_error)?;
            return Err(wallet_error(
                "The app vault changed; unlock or review again",
                true,
            ));
        }
        let was_open = state.opened.is_some();
        state.enforce(Instant::now());
        if was_open && state.opened.is_none() {
            self.epoch.fetch_add(1, Ordering::AcqRel);
            self.work.cancel();
            self.notify_locked();
        }
        if self.revoked.load(Ordering::Acquire) {
            state.lock();
        }
        Ok((state.document.clone(), state.selected.clone()))
    }
    pub(crate) fn public_state(&self) -> Result<PublicWalletState, IpcError> {
        self.snapshot()?;
        let state = self.state.lock().map_err(|_| IpcError::internal())?;
        let entry = state
            .document
            .as_ref()
            .map(AppVaultDocument::inventory)
            .transpose()
            .map_err(password_error)?
            .unwrap_or_default()
            .into_iter()
            .find(|(id, _)| Some(id) == state.selected.as_ref());
        Ok(entry.map_or_else(empty_public, |(id, account)| {
            public(
                &id,
                &account,
                state.opened.is_some() && !self.revoked.load(Ordering::Acquire),
            )
        }))
    }
    pub(super) fn inventory(&self) -> Result<Vec<PublicWalletState>, IpcError> {
        let (document, _) = self.snapshot()?;
        let ready = self
            .state
            .lock()
            .map_err(|_| IpcError::internal())?
            .opened
            .is_some()
            && !self.revoked.load(Ordering::Acquire);
        Ok(document
            .map(|d| d.inventory())
            .transpose()
            .map_err(password_error)?
            .unwrap_or_default()
            .into_iter()
            .map(|(id, a)| public(&id, &a, ready))
            .collect())
    }
    pub(crate) fn selected_address(&self) -> Result<String, IpcError> {
        self.public_state()?
            .address
            .ok_or_else(|| wallet_error("Create, restore or import an account first", false))
    }
    pub(crate) fn handle_system_sleep(&self) {
        self.lock();
    }
    pub(crate) fn lock(&self) {
        self.epoch.fetch_add(1, Ordering::AcqRel);
        self.revoked.store(true, Ordering::Release);
        self.work.cancel();
        self.notify_locked();
        if let Ok(mut state) = self.state.try_lock() {
            state.lock();
        } else if !self.cleanup_pending.swap(true, Ordering::AcqRel) {
            let control = self.clone();
            tauri::async_runtime::spawn_blocking(move || {
                if let Ok(mut state) = control.state.lock() {
                    state.lock();
                }
                control.cleanup_pending.store(false, Ordering::Release);
            });
        }
    }
    pub(crate) fn enforce_inactivity(&self, now: Instant) {
        if let Ok(mut state) = self.state.try_lock() {
            let was_open = state.opened.is_some();
            state.enforce(now);
            if was_open && state.opened.is_none() {
                self.revoked.store(true, Ordering::Release);
                self.epoch.fetch_add(1, Ordering::AcqRel);
                self.work.cancel();
                self.notify_locked();
            }
        }
    }
    pub(super) fn install(
        &self,
        document: AppVaultDocument,
        opened: AppVaultOpen,
        id: VaultKeyId,
        job: &PasswordVaultJob,
    ) -> Result<(), Error> {
        let now = Instant::now();
        let mut session = locked_session(id.clone())?;
        drop(opened.activate(&id, &mut session, now, job)?);
        let expected = document.clone();
        let result = self.store.with_revision(&expected, || {
            job.with_current(|| {
                if self.revoked.load(Ordering::Acquire) {
                    return Err(Error::Cancelled);
                }
                let mut state = self.state.lock().map_err(|_| Error::Cancelled)?;
                self.epoch.fetch_add(1, Ordering::AcqRel);
                self.missing_vault.store(false, Ordering::Release);
                *state = LiveVault {
                    document: Some(document),
                    selected: Some(id),
                    opened: Some(opened),
                    session: Some(session),
                    last_activity: Some(now),
                };
                Ok(())
            })
        });
        if result.is_err()
            && let Ok(mut state) = self.state.lock()
        {
            state.lock();
        }
        result
    }
    pub(super) fn unlock_blocking(
        &self,
        document: AppVaultDocument,
        id: VaultKeyId,
        password: &WalletPassword,
        job: &PasswordVaultJob,
    ) -> Result<(), Error> {
        let opened = document.authenticate(password, job)?;
        self.install(document, opened, id, job)
    }
    pub(super) fn publish(
        &self,
        previous: Option<&AppVaultDocument>,
        document: AppVaultDocument,
        password: &WalletPassword,
        id: VaultKeyId,
        job: &PasswordVaultJob,
    ) -> Result<(), Error> {
        let opened = document.authenticate(password, job)?;
        let result = if let Some(old) = previous {
            self.store.replace(old, &document, job)
        } else {
            self.store.create(&document, job)
        };
        if result.is_err() {
            if let Ok(mut state) = self.state.lock() {
                state.lock();
            }
            return result;
        }
        self.install(document, opened, id, job)
    }
    pub(super) fn select(&self, requested: &WalletId) -> Result<(), IpcError> {
        self.epoch.fetch_add(1, Ordering::AcqRel);
        self.work.cancel();
        let job = self.begin()?;
        self.snapshot()?;
        let id = VaultKeyId::new(&requested.0)
            .map_err(|_| IpcError::invalid_request("Invalid account identifier"))?;
        let mut state = self.state.lock().map_err(|_| IpcError::internal())?;
        let document = state
            .document
            .clone()
            .ok_or_else(|| wallet_error("No app vault is configured", false))?;
        if !document
            .inventory()
            .map_err(password_error)?
            .iter()
            .any(|(entry, _)| entry == &id)
        {
            return Err(wallet_error("The selected account is unavailable", false));
        }
        let mut session = locked_session(id.clone()).map_err(password_error)?;
        if let (Some(opened), Some(last)) = (&state.opened, state.last_activity) {
            drop(
                opened
                    .activate(&id, &mut session, last, &job)
                    .map_err(password_error)?,
            );
        }
        state.selected = Some(id);
        state.session = Some(session);
        Ok(())
    }
}
pub(super) fn locked_session(id: VaultKeyId) -> Result<VaultSession, Error> {
    Ok(VaultSession::new(
        id,
        VaultInactivityTimeout::new(Duration::from_mins(5)).map_err(|_| Error::InvalidDocument)?,
    ))
}
fn empty_public() -> PublicWalletState {
    PublicWalletState {
        wallet_id: None,
        address: None,
        network: None,
        availability: WalletAvailability::Unavailable,
    }
}
fn public(id: &VaultKeyId, a: &wallet_core::AccountMetadata, ready: bool) -> PublicWalletState {
    PublicWalletState {
        wallet_id: Some(WalletId(id.as_str().to_owned())),
        // Locked envelope metadata is not authenticated. Never present it as a deposit destination.
        address: ready.then(|| a.address().to_owned()),
        network: Some(format!("eip155:{}", a.chain_id())),
        availability: if ready {
            WalletAvailability::Ready
        } else {
            WalletAvailability::Locked
        },
    }
}

#[cfg(test)]
#[path = "control/tests.rs"]
mod tests;
