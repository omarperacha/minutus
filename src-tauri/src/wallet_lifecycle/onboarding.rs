//! Native first setup, add, restore and import; passwords never cross IPC.
use super::{
    WalletControl, blocking, cancelled_error,
    native::{self, PasswordPrompt},
    password_error, wallet_error,
};
use ipc_types::IpcError;
use wallet_core::{
    AppVaultDocument, AppVaultEntry, PasswordVaultJob, RecoveryPhrase, RecoverySeed,
    RecoveryWordAnswers, VaultKeyId, WalletPassword,
};
use zeroize::Zeroizing;

pub(super) struct Preparation {
    pub(super) control: WalletControl,
    pub(super) previous: Option<AppVaultDocument>,
    pub(super) password: WalletPassword,
    pub(super) job: PasswordVaultJob,
}
pub(super) async fn password(
    app: &tauri::AppHandle,
    prompt: PasswordPrompt,
    job: &PasswordVaultJob,
) -> Result<WalletPassword, IpcError> {
    job.check().map_err(password_error)?;
    let password = native::request_password(app, prompt)
        .await?
        .ok_or_else(cancelled_error)?;
    job.check().map_err(password_error)?;
    Ok(password)
}
pub(super) async fn confirmed_password(
    app: &tauri::AppHandle,
    job: &PasswordVaultJob,
) -> Result<WalletPassword, IpcError> {
    let first = password(app, PasswordPrompt::Create, job).await?;
    let second = password(app, PasswordPrompt::Confirm, job).await?;
    if !first.matches(&second) {
        return Err(wallet_error(native::PASSWORD_MISMATCH_MESSAGE, false));
    }
    Ok(first)
}
pub(super) async fn prepare(
    app: &tauri::AppHandle,
    control: WalletControl,
) -> Result<Preparation, IpcError> {
    let job = control.begin()?;
    let (previous, _) = control.snapshot()?;
    if previous.is_none() {
        if control.missing_vault.load(std::sync::atomic::Ordering::Acquire) {
            if !native::confirm(app,"Previously configured vault is missing","Minutus remembers an account but its app-vault file is missing. Cancel to locate the original file. Continuing starts a separate fresh vault; recover every required account from its own recovery phrase or private-key backup. This does not restore missing accounts automatically.","Start fresh with my backups").await? {return Err(cancelled_error());}
        } else if control.old_format_present && !native::confirm(app,"Start a password-protected wallet","An incompatible older wallet folder exists. It and its Keychain items will stay untouched. This creates a separate app vault. Recover accounts explicitly from their own backup; nothing is migrated.","Create separate app vault").await? {return Err(cancelled_error());}
    }
    let entered = if previous.is_some() {
        password(app, PasswordPrompt::Existing, &job).await?
    } else {
        confirmed_password(app, &job).await?
    };
    if let Some(document) = previous.clone() {
        let checking = control.clone();
        let (entered, job) = blocking(move || {
            checking.store.with_revision(&document, || {
                drop(document.authenticate(&entered, &job)?);
                Ok(())
            })?;
            Ok((entered, job))
        })
        .await?;
        Ok(Preparation {
            control,
            previous,
            password: entered,
            job,
        })
    } else {
        Ok(Preparation {
            control,
            previous,
            password: entered,
            job,
        })
    }
}
pub(super) async fn verified_seed(
    app: &tauri::AppHandle,
    job: &PasswordVaultJob,
) -> Result<RecoverySeed, IpcError> {
    job.check().map_err(password_error)?;
    let phrase = RecoveryPhrase::generate().map_err(|_| IpcError::internal())?;
    let mut phrase = native::present_recovery_phrase(app, phrase)
        .await?
        .ok_or_else(cancelled_error)?;
    job.check().map_err(password_error)?;
    let prompt = phrase
        .begin_verification()
        .map_err(|_| IpcError::internal())?;
    if !native::acknowledge_recovery_warnings(app, prompt.warnings()).await? {
        return Err(cancelled_error());
    }
    let ack = prompt
        .acknowledge_warnings(true, true, true)
        .map_err(|_| IpcError::internal())?;
    let mut answers = Zeroizing::new(Vec::with_capacity(3));
    for position in prompt.positions() {
        job.check().map_err(password_error)?;
        let mut answer = native::request_secret_owned(
            app,
            "Verify recovery phrase",
            &format!("Enter word {}", position.number()),
        )
        .await?
        .ok_or_else(cancelled_error)?;
        answers.push(std::mem::take(&mut *answer));
    }
    job.check().map_err(password_error)?;
    let words: [String; 3] = std::mem::take(&mut *answers)
        .try_into()
        .map_err(|_| IpcError::internal())?;
    phrase
        .verify_recovery_words(
            &prompt,
            &ack,
            RecoveryWordAnswers::new(words)
                .map_err(|_| wallet_error("Invalid recovery words", false))?,
        )
        .map_err(|_| wallet_error("Recovery verification failed; start again", false))?;
    phrase.derive_seed().map_err(|_| IpcError::internal())
}
pub(super) fn entry_id() -> Result<VaultKeyId, IpcError> {
    VaultKeyId::new(format!("account-{}", uuid::Uuid::now_v7())).map_err(|_| IpcError::internal())
}
pub(super) async fn create(app: &tauri::AppHandle, control: WalletControl) -> Result<(), IpcError> {
    let prep = prepare(app, control).await?;
    let seed = verified_seed(app, &prep.job).await?;
    let id = entry_id()?;
    blocking(move || {
        let account = wallet_core::derive_default_evm_account(&seed)
            .map_err(|_| wallet_core::PasswordVaultError::InvalidDocument)?
            .metadata("account-1", 8453)
            .map_err(|_| wallet_core::PasswordVaultError::InvalidDocument)?;
        let entry = AppVaultEntry::recovery(id, &seed, account)?;
        add(&prep, entry)
    })
    .await
}
pub(super) fn add(
    prep: &Preparation,
    entry: AppVaultEntry,
) -> Result<(), wallet_core::PasswordVaultError> {
    prep.job.check()?;
    let id = entry.id().clone();
    let document = if let Some(previous) = &prep.previous {
        let mut opened = previous.authenticate(&prep.password, &prep.job)?;
        opened.add(entry)?;
        AppVaultDocument::seal(&opened, &prep.password, &prep.job)?
    } else {
        AppVaultDocument::create(
            VaultKeyId::new(format!("app-{}", uuid::Uuid::now_v7()))
                .map_err(|_| wallet_core::PasswordVaultError::InvalidDocument)?,
            entry,
            &prep.password,
            &prep.job,
        )?
    };
    prep.control.publish(
        prep.previous.as_ref(),
        document,
        &prep.password,
        id,
        &prep.job,
    )
}
pub(super) async fn restore(
    app: &tauri::AppHandle,
    control: WalletControl,
) -> Result<(), IpcError> {
    let prep = prepare(app, control).await?;
    let inventory = wallet_core::VaultInventory::new(
        prep.previous
            .as_ref()
            .map(AppVaultDocument::inventory)
            .transpose()
            .map_err(password_error)?
            .unwrap_or_default()
            .into_iter()
            .map(|(id, _)| id)
            .collect(),
    )
    .map_err(|_| IpcError::internal())?;
    let mut words = native::request_secret_owned(
        app,
        "Restore recovery phrase",
        "Enter the complete 24-word recovery phrase",
    )
    .await?
    .ok_or_else(cancelled_error)?;
    let preview = wallet_core::WalletRestorePreview::validate(
        wallet_core::RecoveryPhraseInput::new(std::mem::take(&mut *words))
            .map_err(|_| wallet_error("Recovery phrase is invalid", false))?,
        inventory.clone(),
    )
    .map_err(|_| wallet_error("Recovery phrase is invalid", false))?;
    prep.job.check().map_err(password_error)?;
    if !native::confirm_public_address(
        app,
        "Restore this account?",
        preview.address().as_str(),
        wallet_core::DEFAULT_EVM_ACCOUNT_RECOVERY_GUIDANCE,
    )
    .await?
    {
        return Err(cancelled_error());
    }
    let new_id = entry_id()?;
    let choice = wallet_core::WalletRestoreChoice::Add {
        new_vault_id: new_id.clone(),
    };
    let confirmed = preview
        .confirm(choice)
        .map_err(|_| wallet_error("Restore destination changed", false))?;
    let commit = confirmed
        .authorize_commit(&inventory)
        .map_err(|_| wallet_error("Restore destination changed", false))?;
    let id = new_id;
    let seed = commit.into_seed();
    blocking(move || {
        let account = wallet_core::derive_default_evm_account(&seed)
            .map_err(|_| wallet_core::PasswordVaultError::InvalidDocument)?
            .metadata("account-1", 8453)
            .map_err(|_| wallet_core::PasswordVaultError::InvalidDocument)?;
        let entry = AppVaultEntry::recovery(id, &seed, account)?;
        add(&prep, entry)
    })
    .await
}
pub(super) async fn imported_entry(
    app: &tauri::AppHandle,
    job: &PasswordVaultJob,
) -> Result<AppVaultEntry, IpcError> {
    let mut value = native::request_secret_owned(
        app,
        "Import private key",
        "Enter a 32-byte hexadecimal EVM private key",
    )
    .await?
    .ok_or_else(cancelled_error)?;
    let preview = wallet_core::EvmPrivateKeyImportPreview::validate(
        wallet_core::EvmPrivateKeyInput::new(std::mem::take(&mut *value))
            .map_err(|_| wallet_error("Private key is invalid", false))?,
    )
    .map_err(|_| wallet_error("Private key is invalid", false))?;
    job.check().map_err(password_error)?;
    if !native::confirm_public_address(
        app,
        "Import this account?",
        preview.address().as_str(),
        preview.recovery_warning(),
    )
    .await?
    {
        return Err(cancelled_error());
    }
    let metadata = wallet_core::AccountMetadata::new(
        "account-1",
        preview.address().as_str(),
        8453,
        wallet_core::AccountKeyOrigin::ImportedPrivateKey,
    )
    .map_err(|_| IpcError::internal())?;
    let ack = preview
        .acknowledge_recovery_warning(true)
        .map_err(|_| IpcError::internal())?;
    AppVaultEntry::imported(
        entry_id()?,
        preview.confirm(&ack).map_err(|_| IpcError::internal())?,
        metadata,
    )
    .map_err(password_error)
}
pub(super) async fn import(app: &tauri::AppHandle, control: WalletControl) -> Result<(), IpcError> {
    let prep = prepare(app, control).await?;
    let entry = imported_entry(app, &prep.job).await?;
    blocking(move || add(&prep, entry)).await
}
