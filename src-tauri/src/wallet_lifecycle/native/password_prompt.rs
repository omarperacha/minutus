//! Host-chosen password purposes; none can be selected by model arguments.
use ipc_types::IpcError;

pub(crate) enum PasswordPrompt {
    Enter,
    Unlock,
    SignTransaction,
    SignPayment,
    Existing,
    Create,
    Confirm,
}

pub(super) struct Presentation {
    pub(super) title: &'static str,
    pub(super) instructions: &'static str,
    pub(super) accept: &'static str,
}

impl PasswordPrompt {
    pub(super) fn presentation(&self) -> Presentation {
        let (title, instructions, accept) = match self {
            Self::Unlock => (
                "Unlock wallet",
                "Enter your app password to unlock the selected wallet. This does not approve or sign a transaction. Every transaction requires its own exact review and a separate fresh password.",
                "Unlock wallet",
            ),
            Self::SignTransaction => (
                "Authorise approved transaction",
                "Enter your app password again to authorise only the exact transaction you just approved. This fresh authentication is separate from unlocking the wallet. After acceptance, verification and signing may continue while you use another app. Locking, sleep, wallet changes, cancellation or expiry still stop the attempt.",
                "Authorise this transaction",
            ),
            Self::SignPayment => (
                "Authorise approved payment",
                "Enter your app password again to authorise only the exact payment you just approved. This fresh authentication is separate from unlocking the wallet. After acceptance, verification and signing may continue while you use another app. Locking, sleep, wallet changes, cancellation or expiry still stop the attempt.",
                "Authorise this payment",
            ),
            Self::Enter => ("Enter password", "", "Continue"),
            Self::Existing => ("Enter existing password", "", "Continue"),
            Self::Create => (
                "Enter new password",
                "Use at least 15 characters. Spaces and Unicode are allowed. Maximum 1024 UTF-8 bytes. A copied vault can be guessed offline; use a strong, unique passphrase. The password cannot recover lost keys; keep a separate backup for every recovery phrase and imported private key.",
                "Continue",
            ),
            Self::Confirm => (
                "Confirm new password",
                "Enter the new password again.",
                "Continue",
            ),
        };
        Presentation {
            title,
            instructions,
            accept,
        }
    }
    pub(super) fn input_error(&self, message: &str) -> IpcError {
        super::super::wallet_error(
            match self {
                Self::Confirm => PASSWORD_MISMATCH_MESSAGE,
                Self::Create => message,
                _ => "Password wallet could not be authenticated",
            },
            false,
        )
    }
}

pub(crate) const PASSWORD_MISMATCH_MESSAGE: &str = "Passwords do not match; start again";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlock_and_fresh_authorisation_have_distinct_host_owned_purposes() {
        let unlock = PasswordPrompt::Unlock.presentation();
        let signing = PasswordPrompt::SignTransaction.presentation();
        let payment = PasswordPrompt::SignPayment.presentation();
        assert_ne!(unlock.title, signing.title);
        assert_ne!(signing.title, payment.title);
        assert!(unlock.instructions.contains("does not approve or sign"));
        assert!(
            signing
                .instructions
                .contains("exact transaction you just approved")
        );
        assert!(
            payment
                .instructions
                .contains("exact payment you just approved")
        );
        assert!(signing.instructions.contains("separate from unlocking"));
        assert!(signing.instructions.contains("another app"));
        assert_eq!(signing.accept, "Authorise this transaction");
    }

    #[test]
    fn invalid_confirmation_reports_mismatch_while_new_password_keeps_policy_guidance()
    -> Result<(), Box<dyn std::error::Error>> {
        for input in [String::new(), "short".to_owned(), "x".repeat(1025)] {
            let failure = wallet_core::WalletPassword::new(input)
                .err()
                .ok_or("synthetic invalid input was accepted")?;
            assert_eq!(
                PasswordPrompt::Confirm
                    .input_error(&failure.to_string())
                    .message,
                PASSWORD_MISMATCH_MESSAGE
            );
            assert_eq!(
                PasswordPrompt::Create
                    .input_error(&failure.to_string())
                    .message,
                failure.to_string()
            );
        }
        assert_eq!(
            PasswordPrompt::Confirm
                .input_error("Password is too long")
                .message,
            PASSWORD_MISMATCH_MESSAGE
        );
        Ok(())
    }
}
