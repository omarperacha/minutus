//! Content-free native wallet error projection.
use ipc_types::{IpcError, IpcErrorCode};
pub(crate) fn wallet_error(message: &str, retryable: bool) -> IpcError {
    IpcError {
        code: IpcErrorCode::Unsupported,
        message: message.to_owned(),
        retryable,
    }
}
pub(super) fn cancelled_error() -> IpcError {
    IpcError {
        code: IpcErrorCode::Cancelled,
        message: "Wallet operation was cancelled".to_owned(),
        retryable: false,
    }
}
pub(super) fn password_error(error: wallet_core::PasswordVaultError) -> IpcError {
    if error == wallet_core::PasswordVaultError::Cancelled {
        cancelled_error()
    } else {
        wallet_error(
            &error.to_string(),
            matches!(
                error,
                wallet_core::PasswordVaultError::Busy
                    | wallet_core::PasswordVaultError::RateLimited
                    | wallet_core::PasswordVaultError::Conflict
            ),
        )
    }
}
