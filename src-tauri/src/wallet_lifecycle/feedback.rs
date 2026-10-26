//! Native acknowledgement of sanitised failures, without overstating commit outcome.
use ipc_types::{IpcError, IpcErrorCode};

pub(super) async fn report_failure(app: &tauri::AppHandle, operation: &str, error: &IpcError) {
    if let Some((title, body)) = failure_notice(operation, error) {
        // Keep the original IPC error for inline fallback if native presentation fails.
        let _ = super::native::show_message(app, &title, &body).await;
    }
}

fn failure_notice(operation: &str, error: &IpcError) -> Option<(String, String)> {
    if error.code == IpcErrorCode::Cancelled {
        return None;
    }
    let authentication =
        error.message == wallet_core::PasswordVaultError::AuthenticationFailed.to_string();
    let input_failure = authentication
        || error.message == super::native::PASSWORD_MISMATCH_MESSAGE
        || error.message == wallet_core::PasswordVaultError::InvalidPassword.to_string();
    if input_failure {
        let body = if authentication {
            "The password could not be verified. Try again with your existing app password."
                .to_owned()
        } else {
            error.message.clone()
        };
        return Some((format!("{operation} failed"), body));
    }
    // A storage or post-publication failure may have changed persisted state.
    // Never turn an uncertain outcome into a claim that nothing was changed.
    Some((
        format!("{operation} could not be confirmed"),
        format!(
            "{}\n\nCheck the current wallet state before trying again.",
            error.message
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authentication_and_mismatch_are_explicit_failures_but_uncertain_commit_is_not()
    -> Result<(), Box<dyn std::error::Error>> {
        let auth =
            super::super::password_error(wallet_core::PasswordVaultError::AuthenticationFailed);
        let (title, body) = failure_notice("Account removal", &auth).ok_or("missing notice")?;
        assert_eq!(title, "Account removal failed");
        assert!(body.contains("password could not be verified"));
        let mismatch =
            super::super::wallet_error(super::super::native::PASSWORD_MISMATCH_MESSAGE, false);
        let (title, body) = failure_notice("Password change", &mismatch).ok_or("missing notice")?;
        assert_eq!(title, "Password change failed");
        assert!(body.contains("Passwords do not match"));
        let uncertain =
            super::super::password_error(wallet_core::PasswordVaultError::CommitUncertain);
        let (title, body) =
            failure_notice("Account removal", &uncertain).ok_or("missing notice")?;
        assert_eq!(title, "Account removal could not be confirmed");
        assert!(body.contains("durability is uncertain"));
        assert!(!body.contains("unchanged"));
        assert!(failure_notice("Account removal", &super::super::cancelled_error()).is_none());
        Ok(())
    }
}
