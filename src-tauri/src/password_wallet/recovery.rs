//! Native phrase restore for an empty, isolated test-wallet slot.
use super::{control::Control, flow::confirmed_password};
use crate::wallet_lifecycle::{native, wallet_error};
use ipc_types::IpcError;
use std::sync::Arc;
use wallet_core::{
    PasswordVaultJob, RecoveryPhraseInput, VaultKeyId, WalletRestoreChoice, WalletRestorePreview,
};

pub(super) async fn restore(
    app: &tauri::AppHandle,
    control: Arc<Control>,
    job: PasswordVaultJob,
) -> Result<(), IpcError> {
    let inventory = control.recovery_inventory().map_err(|failure| {
        let message = if failure == wallet_core::PasswordVaultError::Conflict {
            "Restore is available after deleting the current test wallet".to_owned()
        } else {
            failure.to_string()
        };
        wallet_error(&message, false)
    })?;
    job.check().map_err(|_| cancelled())?;
    let mut words = native::request_secret_owned(
        app,
        "Restore from recovery phrase",
        "Enter all recovery words for your unfunded test wallet in order, separated by spaces. You will choose a new wallet password after confirming the recovered address.",
    )
    .await?
    .ok_or_else(cancelled)?;
    job.check().map_err(|_| cancelled())?;
    let input =
        RecoveryPhraseInput::new(std::mem::take(&mut *words)).map_err(|_| invalid_phrase())?;
    let preview = WalletRestorePreview::validate(input, inventory).map_err(|_| invalid_phrase())?;
    job.check().map_err(|_| cancelled())?;
    if !native::confirm(
        app,
        "Confirm recovered wallet",
        &format!(
            "Recovered address\n{}\n\nCheck that this matches the wallet you deleted. A recovery phrase restores its derived account; imported private-key accounts need their own backup.",
            preview.address().as_str(),
        ),
        "Restore this account",
    )
    .await?
    {
        return Err(cancelled());
    }
    job.check().map_err(|_| cancelled())?;
    let password = confirmed_password(app, &job).await?;
    let id = VaultKeyId::new(format!("password-test-{}", uuid::Uuid::now_v7()))
        .map_err(|_| IpcError::internal())?;
    let confirmed = preview
        .confirm(WalletRestoreChoice::Add { new_vault_id: id })
        .map_err(|_| IpcError::internal())?;
    tauri::async_runtime::spawn_blocking(move || control.restore(confirmed, &password, &job))
        .await
        .map_err(|_| IpcError::internal())?
        .map_err(|failure| wallet_error(&failure.to_string(), false))
}

fn cancelled() -> IpcError {
    wallet_error("Wallet recovery was cancelled or locked", false)
}
fn invalid_phrase() -> IpcError {
    wallet_error(
        "Recovery phrase is invalid; check the words and their order",
        false,
    )
}
