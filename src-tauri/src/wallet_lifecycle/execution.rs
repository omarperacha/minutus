//! Blocking-executor bridge for admitted native password work.
use super::password_error;
use ipc_types::IpcError;
pub(super) async fn blocking<T: Send + 'static>(
    effect: impl FnOnce() -> Result<T, wallet_core::PasswordVaultError> + Send + 'static,
) -> Result<T, IpcError> {
    tauri::async_runtime::spawn_blocking(effect)
        .await
        .map_err(|_| IpcError::internal())?
        .map_err(password_error)
}
