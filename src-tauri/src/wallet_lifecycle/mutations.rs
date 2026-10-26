//! Native global password changes, exact removals and explicit recovery reset.
use super::{
    WalletControl, blocking, cancelled_error,
    native::{self, PasswordPrompt},
    onboarding, password_error, wallet_error,
};
use ipc_types::IpcError;
use wallet_core::{
    AppVaultDocument, AppVaultEntry, PasswordVaultError as Error, VaultDeletionPreview,
    VaultRecoveryConfirmation, VaultTypedConfirmation,
};

pub(super) async fn change_password(
    app: &tauri::AppHandle,
    control: WalletControl,
) -> Result<(), IpcError> {
    let job = control.begin()?;
    let (document, _) = control.snapshot()?;
    let previous = document.ok_or_else(|| wallet_error("No app vault is configured", false))?;
    let current = onboarding::password(app, PasswordPrompt::Existing, &job).await?;
    let check = previous.clone();
    let checking = control.clone();
    let (current, job) = blocking(move || {
        checking.store.with_revision(&check, || {
            drop(check.authenticate(&current, &job)?);
            Ok(())
        })?;
        Ok((current, job))
    })
    .await?;
    let replacement = onboarding::confirmed_password(app, &job).await?;
    if !native::confirm(app,"Change app password","This changes the password for every account and locks the app vault. Old encrypted copies may still open with the old password. Recovery phrases, imported keys and on-chain accounts do not change.","Change password for all accounts").await? {return Err(cancelled_error());}
    blocking(move || {
        let changed = previous.change_password(&current, &replacement, &job)?;
        if let Ok(mut state) = control.state.lock() {
            state.lock();
        }
        let result = control.store.replace(&previous, &changed, &job);
        if result.is_ok() {
            let mut state = control.state.lock().map_err(|_| Error::Cancelled)?;
            state.document = Some(changed);
        }
        result
    })
    .await
}
pub(super) async fn remove_account(
    app: &tauri::AppHandle,
    control: WalletControl,
) -> Result<(), IpcError> {
    let job = control.begin()?;
    let (document, selected) = control.snapshot()?;
    let previous = document.ok_or_else(|| wallet_error("No app vault is configured", false))?;
    let id = selected.ok_or_else(|| wallet_error("Select an account first", false))?;
    let account = previous
        .inventory()
        .map_err(password_error)?
        .into_iter()
        .find(|(entry, _)| entry == &id)
        .ok_or_else(IpcError::internal)?
        .1;
    let warning = match account.key_origin() {
        wallet_core::AccountKeyOrigin::ImportedPrivateKey => {
            "This imported account needs its own private-key backup; a recovery phrase cannot restore it."
        }
        wallet_core::AccountKeyOrigin::RecoveryPhrase { .. } => {
            "This account needs its own recovery phrase. Other phrases cannot restore it."
        }
    };
    if !native::request_confirmation(app,"Remove this account",&format!("REMOVE {}",id.as_str()),&format!("Address: {}\n\n{warning}\n\nConfirm you have its backup or accept permanent loss. Other accounts stay in the app vault. Removing the final account deletes the local app vault. External copies are not erased. You will enter the app password next.",account.address())).await? {return Err(cancelled_error());}
    let password = onboarding::password(app, PasswordPrompt::Enter, &job).await?;
    blocking(move || {
        let mut opened = previous.authenticate(&password, &job)?;
        opened.remove(&id)?;
        if opened.entries().is_empty() {
            {
                let mut state = control.state.lock().map_err(|_| Error::Cancelled)?;
                state.lock();
            }
            control.store.remove_empty(&previous, opened, &job)?;
            let mut state = control.state.lock().map_err(|_| Error::Cancelled)?;
            control
                .missing_vault
                .store(false, std::sync::atomic::Ordering::Release);
            state.document = None;
            state.selected = None;
            state.session = None;
            Ok(())
        } else {
            let next = opened.entries()[0].id().clone();
            let changed = AppVaultDocument::seal(&opened, &password, &job)?;
            control.publish(Some(&previous), changed, &password, next, &job)
        }
    })
    .await
}
pub(super) async fn delete(app: &tauri::AppHandle, control: WalletControl) -> Result<(), IpcError> {
    let job = control.begin()?;
    let (document, _) = control.snapshot()?;
    let previous = document.ok_or_else(|| wallet_error("No app vault is configured", false))?;
    let preview = VaultDeletionPreview::new_password_protected(previous.id().clone());
    let accounts = previous
        .inventory()
        .map_err(password_error)?
        .iter()
        .map(|(_, a)| a.address().to_owned())
        .collect::<Vec<_>>()
        .join("\n");
    let required = preview.required_confirmation().to_owned();
    if !native::request_confirmation(app,"Delete the entire app vault",&required,&format!("Every account below will be removed locally:\n\n{accounts}\n\nConfirm you have every separate recovery phrase and imported private key, or accept permanent loss. One phrase does not restore unrelated accounts. External copies are not erased. You will enter the app password next.")).await? {return Err(cancelled_error());}
    let commit = preview
        .confirm(
            VaultRecoveryConfirmation::RecoveryUnavailableAndPermanentLossAccepted,
            true,
            VaultTypedConfirmation::new(required).map_err(|_| IpcError::internal())?,
        )
        .map_err(|_| IpcError::internal())?;
    let password = onboarding::password(app, PasswordPrompt::Enter, &job).await?;
    blocking(move || {
        {
            let mut state = control.state.lock().map_err(|_| Error::Cancelled)?;
            state.lock();
        }
        control.store.delete(&previous, &password, commit, &job)?;
        let mut state = control.state.lock().map_err(|_| Error::Cancelled)?;
        control
            .missing_vault
            .store(false, std::sync::atomic::Ordering::Release);
        state.document = None;
        state.selected = None;
        state.session = None;
        Ok(())
    })
    .await
}
pub(super) async fn reset(
    app: &tauri::AppHandle,
    control: WalletControl,
    private_key: bool,
) -> Result<(), IpcError> {
    let job = control.begin()?;
    let (document, _) = control.snapshot()?;
    let previous = document.ok_or_else(|| {
        wallet_error(
            "No app vault is configured; use normal recovery or import",
            false,
        )
    })?;
    let entry = if private_key {
        onboarding::imported_entry(app, &job).await?
    } else {
        let mut words = native::request_secret_owned(
            app,
            "Reset and recover",
            "Enter the complete 24-word recovery phrase for the account you want to recover",
        )
        .await?
        .ok_or_else(cancelled_error)?;
        let preview = wallet_core::WalletRestorePreview::validate(
            wallet_core::RecoveryPhraseInput::new(std::mem::take(&mut *words))
                .map_err(|_| wallet_error("Recovery phrase is invalid", false))?,
            wallet_core::VaultInventory::empty(),
        )
        .map_err(|_| wallet_error("Recovery phrase is invalid", false))?;
        if !native::confirm_public_address(app,"Recover this account?",preview.address().as_str(),"Only this phrase's account will be recovered. Every other account needs its own backup.").await? {return Err(cancelled_error());}
        let id = onboarding::entry_id()?;
        let commit = preview
            .confirm(wallet_core::WalletRestoreChoice::Add {
                new_vault_id: id.clone(),
            })
            .map_err(|_| IpcError::internal())?
            .authorize_commit(&wallet_core::VaultInventory::empty())
            .map_err(|_| IpcError::internal())?;
        let seed = commit.into_seed();
        let metadata = wallet_core::derive_default_evm_account(&seed)
            .map_err(|_| IpcError::internal())?
            .metadata("account-1", 8453)
            .map_err(|_| IpcError::internal())?;
        AppVaultEntry::recovery(id, &seed, metadata).map_err(password_error)?
    };
    let password = onboarding::confirmed_password(app, &job).await?;
    let required = format!("RESET {}", previous.id().as_str());
    if !native::request_confirmation(app,"Replace the inaccessible app vault",&required,"This replaces the ENTIRE local app vault with only the recovered account. All other accounts will be lost unless you separately backed up their phrases/private keys. The old password cannot recover them from the replacement. Cancel to keep the current vault.").await? {return Err(cancelled_error());}
    let commit = wallet_core::AppVaultResetCommit::confirm(
        &previous,
        true,
        VaultTypedConfirmation::new(required).map_err(|_| IpcError::internal())?,
    )
    .map_err(password_error)?;
    blocking(move || {
        let id = entry.id().clone();
        let changed = AppVaultDocument::create(previous.id().clone(), entry, &password, &job)?;
        let opened = changed.authenticate(&password, &job)?;
        {
            let mut state = control.state.lock().map_err(|_| Error::Cancelled)?;
            state.lock();
        }
        control.store.reset(&previous, &changed, commit, &job)?;
        control.install(changed, opened, id, &job)
    })
    .await
}
