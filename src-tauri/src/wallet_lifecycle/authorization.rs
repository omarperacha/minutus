//! Closed host observations only; no credential, approval or signing authority.
use serde::Serialize;

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum WalletPasswordPurpose {
    Unlock,
    SignTransaction,
    SignPayment,
    OtherWalletOperation,
}
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum WalletPasswordDecision {
    Accepted,
    CancelButton,
    HostAbort,
    OtherDismissal,
}
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum WalletAuthorizationStage {
    BeforePassword,
    AfterPassword,
}
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum WalletAuthenticationFailure {
    Cancelled,
    CredentialsRejected,
    Unavailable,
}
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum WalletAuthorizationEvent {
    PasswordRequested {
        purpose: WalletPasswordPurpose,
    },
    PasswordPresented {
        purpose: WalletPasswordPurpose,
    },
    PasswordDecision {
        purpose: WalletPasswordPurpose,
        decision: WalletPasswordDecision,
    },
    JobInvalidated {
        stage: WalletAuthorizationStage,
    },
    AuthenticationStarted,
    AuthenticationSucceeded,
    AuthenticationFailed {
        failure: WalletAuthenticationFailure,
    },
}

pub(super) fn authentication_failure(
    error: wallet_core::PasswordVaultError,
) -> WalletAuthenticationFailure {
    match error {
        wallet_core::PasswordVaultError::Cancelled => WalletAuthenticationFailure::Cancelled,
        wallet_core::PasswordVaultError::AuthenticationFailed => {
            WalletAuthenticationFailure::CredentialsRejected
        }
        _ => WalletAuthenticationFailure::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_actual_authentication_failure_is_reported_as_rejected_credentials() {
        for (error, expected) in [
            (
                wallet_core::PasswordVaultError::AuthenticationFailed,
                "credentialsRejected",
            ),
            (wallet_core::PasswordVaultError::Cancelled, "cancelled"),
            (wallet_core::PasswordVaultError::Conflict, "unavailable"),
        ] {
            assert_eq!(
                serde_json::to_value(authentication_failure(error)).ok(),
                Some(serde_json::json!(expected))
            );
        }
    }
}
