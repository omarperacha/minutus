//! Native user orchestration; credentials never cross a serialised boundary.
use super::{
    control::Control,
    native::{self, Action},
    synthetic::SyntheticCandidate,
};
use crate::wallet_lifecycle::{native as secret, native::PasswordPrompt, wallet_error};
use app_core::AppCore;
use ipc_types::IpcError;
use std::sync::Arc;
use tauri::Manager as _;
use transaction_policy::{ApprovalCommitment, ApprovalInvalidationReason};
use wallet_core::{
    PasswordVaultError, PasswordVaultJob, RecoveryPhrase, RecoverySeed, RecoveryWordAnswers,
    VaultDeletionPreview, VaultKeyId, VaultRecoveryConfirmation, VaultTypedConfirmation,
    WalletPassword,
};
use zeroize::Zeroizing;

pub(super) async fn open(app: &tauri::AppHandle) {
    let control = app.state::<Arc<Control>>().inner().clone();
    let result = run(app, control).await;
    if let Err(error) = result {
        native::notice(app, error.message).await;
    }
}

async fn run(app: &tauri::AppHandle, control: Arc<Control>) -> Result<(), IpcError> {
    let action = native::choose(app, control.summary()).await?;
    if matches!(action, Action::Lock) {
        super::lock(app);
        return Ok(());
    }
    if matches!(action, Action::Cancel) {
        return Ok(());
    }
    let job = control.begin_operation().map_err(error)?;
    job.check().map_err(error)?;
    match action {
        Action::Create => create(app, control, job).await?,
        Action::Restore => super::recovery::restore(app, control, job).await?,
        Action::Unlock => {
            let password = password(app, PasswordPrompt::Enter, &job).await?;
            blocking(move || control.unlock(&password, &job)).await?;
        }
        Action::ChangePassword => {
            let current = password(app, PasswordPrompt::Existing, &job).await?;
            let verifying_control = control.clone();
            let (current, job) = blocking(move || {
                verifying_control.verify_password_for_change(&current, &job)?;
                Ok((current, job))
            })
            .await?;
            let replacement = confirmed_password(app, &job).await?;
            if !secret::confirm(app,"Change test wallet password","The wallet will lock. This changes the live file only. Old encrypted copies remain unlockable with the old password; recovery material and on-chain keys are unchanged.","Change password").await? {return Err(cancelled());}
            blocking(move || control.change_password(&current, &replacement, &job)).await?;
        }
        Action::Sign => sign(app, control, job).await?,
        Action::Delete => delete(app, control, job).await?,
        Action::Lock | Action::Cancel => {}
    }
    native::notice(
        app,
        if matches!(action,Action::Create|Action::Restore) {
            "Test wallet saved and locked. Choose Unlock before signing synthetic examples. No transaction was broadcast."
        } else {"Test operation completed. No transaction was broadcast."}.to_owned(),
    )
    .await;
    Ok(())
}

async fn password(
    app: &tauri::AppHandle,
    prompt: PasswordPrompt,
    job: &PasswordVaultJob,
) -> Result<WalletPassword, IpcError> {
    job.check().map_err(error)?;
    let password = secret::request_password(app, prompt)
        .await?
        .ok_or_else(cancelled)?;
    job.check().map_err(error)?;
    Ok(password)
}

pub(super) async fn confirmed_password(
    app: &tauri::AppHandle,
    job: &PasswordVaultJob,
) -> Result<WalletPassword, IpcError> {
    let first = password(app, PasswordPrompt::Create, job).await?;
    let second = password(app, PasswordPrompt::Confirm, job).await?;
    if !first.matches(&second) {
        return Err(wallet_error(secret::PASSWORD_MISMATCH_MESSAGE, false));
    }
    Ok(first)
}

async fn verified_seed(
    app: &tauri::AppHandle,
    job: &PasswordVaultJob,
) -> Result<RecoverySeed, IpcError> {
    let phrase = RecoveryPhrase::generate().map_err(|_| IpcError::internal())?;
    let mut phrase = secret::present_recovery_phrase(app, phrase)
        .await?
        .ok_or_else(cancelled)?;
    job.check().map_err(error)?;
    let prompt = phrase
        .begin_verification()
        .map_err(|_| IpcError::internal())?;
    if !secret::acknowledge_recovery_warnings(app, prompt.warnings()).await? {
        return Err(cancelled());
    }
    let acknowledgement = prompt
        .acknowledge_warnings(true, true, true)
        .map_err(|_| IpcError::internal())?;
    let mut answers = Zeroizing::new(Vec::with_capacity(3));
    for position in prompt.positions() {
        job.check().map_err(error)?;
        let mut answer = secret::request_secret_owned(
            app,
            "Verify recovery phrase",
            &format!("Enter word {}", position.number()),
        )
        .await?
        .ok_or_else(cancelled)?;
        answers.push(std::mem::take(&mut *answer));
    }
    job.check().map_err(error)?;
    let answers: [String; 3] = std::mem::take(&mut *answers)
        .try_into()
        .map_err(|_| IpcError::internal())?;
    phrase
        .verify_recovery_words(
            &prompt,
            &acknowledgement,
            RecoveryWordAnswers::new(answers)
                .map_err(|_| wallet_error("Invalid recovery words", false))?,
        )
        .map_err(|_| wallet_error("Recovery verification failed; start again", false))?;
    phrase.derive_seed().map_err(|_| IpcError::internal())
}

async fn create(
    app: &tauri::AppHandle,
    control: Arc<Control>,
    job: PasswordVaultJob,
) -> Result<(), IpcError> {
    match control.id() {
        Ok(_) => return Err(error(PasswordVaultError::Conflict)),
        Err(PasswordVaultError::NotFound) => {}
        Err(failure) => return Err(error(failure)),
    }
    if !secret::confirm(app,"Create an unfunded test wallet","This separate password wallet is for local synthetic examples only. Never fund it. Your existing wallet and Keychain items are untouched.","Create test wallet").await? {return Err(cancelled());}
    let password = confirmed_password(app, &job).await?;
    let seed = verified_seed(app, &job).await?;
    let id = VaultKeyId::new(format!("password-test-{}", uuid::Uuid::now_v7()))
        .map_err(|_| IpcError::internal())?;
    blocking(move || control.create(id, &password, &seed, &job)).await
}

async fn delete(
    app: &tauri::AppHandle,
    control: Arc<Control>,
    job: PasswordVaultJob,
) -> Result<(), IpcError> {
    let preview = VaultDeletionPreview::new_password_protected(control.id().map_err(error)?);
    if !secret::confirm(
        app,
        "Confirm recovery material",
        "Do you have the recovery phrase for this test wallet? If you do not, cancel deletion.",
        "I have the recovery material",
    )
    .await?
    {
        return Err(cancelled());
    }
    if !secret::confirm(
        app,
        "Delete this test wallet",
        preview.warning(),
        "Continue to exact confirmation",
    )
    .await?
    {
        return Err(cancelled());
    }
    let mut typed = native::request_deletion_confirmation(app, preview.required_confirmation())
        .await?
        .ok_or_else(cancelled)?;
    let typed = VaultTypedConfirmation::new(std::mem::take(&mut *typed))
        .map_err(|_| wallet_error("Invalid deletion confirmation", false))?;
    let commit = preview
        .confirm(
            VaultRecoveryConfirmation::AllRecoveryMaterialAvailable,
            true,
            typed,
        )
        .map_err(|_| wallet_error("Deletion confirmation did not match", false))?;
    let password = password(app, PasswordPrompt::Enter, &job).await?;
    blocking(move || control.delete(&password, commit, &job)).await
}

async fn sign(
    app: &tauri::AppHandle,
    control: Arc<Control>,
    job: PasswordVaultJob,
) -> Result<(), IpcError> {
    let core = app.state::<Arc<AppCore>>().inner().clone();
    let [transaction, _] =
        SyntheticCandidate::pair(&control.account().map_err(error)?).map_err(error)?;
    let job = sign_one(app, control.clone(), core.clone(), transaction, job).await?;
    let [_, typed] = SyntheticCandidate::pair(&control.account().map_err(error)?).map_err(error)?;
    sign_one(app, control, core, typed, job).await?;
    Ok(())
}

async fn sign_one(
    app: &tauri::AppHandle,
    control: Arc<Control>,
    core: Arc<AppCore>,
    candidate: SyntheticCandidate,
    job: PasswordVaultJob,
) -> Result<PasswordVaultJob, IpcError> {
    job.check().map_err(error)?;
    if !secret::confirm(
        app,
        "Approve exact synthetic example",
        &format!(
            "{}\n\nNo RPC or broadcast will occur. Approve only these exact facts.",
            candidate.display
        ),
        "Approve this example",
    )
    .await?
    {
        return Err(cancelled());
    }
    let approval = ApprovalCommitment::issue(candidate.binding(), super::synthetic::unix_now())
        .map_err(|_| IpcError::internal())?;
    let password = match password(app, PasswordPrompt::Enter, &job).await {
        Ok(password) => password,
        Err(e) => {
            let _ = approval.invalidate(
                ApprovalInvalidationReason::Cancelled,
                super::synthetic::unix_now(),
                core.as_ref(),
            );
            return Err(e);
        }
    };
    let (result, job) = tauri::async_runtime::spawn_blocking(move || {
        let result = control.sign(&core, &candidate, &approval, &password, &job);
        if result.is_err() {
            let _ = approval.invalidate(
                ApprovalInvalidationReason::Cancelled,
                super::synthetic::unix_now(),
                core.as_ref(),
            );
        }
        (result, job)
    })
    .await
    .map_err(|_| IpcError::internal())?;
    result.map_err(error)?;
    Ok(job)
}

async fn blocking<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T, PasswordVaultError> + Send + 'static,
) -> Result<T, IpcError> {
    tauri::async_runtime::spawn_blocking(operation)
        .await
        .map_err(|_| IpcError::internal())?
        .map_err(error)
}
fn cancelled() -> IpcError {
    error(PasswordVaultError::Cancelled)
}
fn error(error: PasswordVaultError) -> IpcError {
    wallet_error(&error.to_string(), false)
}
