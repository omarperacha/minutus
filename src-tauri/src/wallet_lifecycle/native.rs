//! macOS-owned wallet secret presentation and input surfaces.

use ipc_types::IpcError;
use objc2::{MainThreadMarker, rc::autoreleasepool};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertStyle, NSApplication, NSRunningApplication,
    NSSecureTextField, NSTextField,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use std::{
    thread,
    time::{Duration, Instant},
};
use tokio::sync::oneshot;
use wallet_core::{RecoveryPhrase, RecoveryVerificationWarnings};
use zeroize::Zeroizing;

/// Reports whether Minutus remains the active macOS application.
///
/// The frontmost application is the relevant signing observation. A native
/// authentication window can temporarily remove focus from the webview
/// window without backgrounding Minutus, so window focus is not a sound proxy for
/// this security boundary.
pub(crate) fn application_is_active_after_authorization() -> bool {
    const RESTORATION_TIMEOUT: Duration = Duration::from_secs(1);
    const POLL_INTERVAL: Duration = Duration::from_millis(10);

    let deadline = Instant::now() + RESTORATION_TIMEOUT;
    loop {
        if autoreleasepool(|_| NSRunningApplication::currentApplication().isActive()) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(POLL_INTERVAL);
    }
}

pub(crate) async fn present_recovery_phrase(
    app: &tauri::AppHandle,
    mut phrase: RecoveryPhrase,
) -> Result<Option<RecoveryPhrase>, IpcError> {
    let (sender, receiver) = oneshot::channel();
    app.run_on_main_thread(move || {
        let result = autoreleasepool(|_| {
            let mtm = MainThreadMarker::new().ok_or_else(IpcError::internal)?;
            let accepted = phrase
                .present_once_for_onboarding(|words| {
                    let recovery_text = Zeroizing::new(format!(
                        "Write these 24 words down in order. They will not be shown again.\n\n{words}\n\nNever share this phrase with anyone."
                    ));
                    let alert = NSAlert::new(mtm);
                    alert.setAlertStyle(NSAlertStyle::Warning);
                    alert.setMessageText(&NSString::from_str("Save your recovery phrase"));
                    alert.setInformativeText(&NSString::from_str(&recovery_text));
                    alert.addButtonWithTitle(&NSString::from_str("I saved it securely"));
                    alert.addButtonWithTitle(&NSString::from_str("Cancel"));
                    let accepted=alert.runModal() == NSAlertFirstButtonReturn;
                    alert.setInformativeText(&NSString::new());
                    alert.window().close();
                    accepted
                })
                .map_err(|_| IpcError::internal())?;
            Ok(accepted.then_some(phrase))
        });
        let _ = sender.send(result);
    }).map_err(|_| IpcError::internal())?;
    receiver.await.map_err(|_| IpcError::internal())?
}

pub(crate) async fn acknowledge_recovery_warnings(
    app: &tauri::AppHandle,
    warnings: RecoveryVerificationWarnings,
) -> Result<bool, IpcError> {
    confirm(
        app,
        "Acknowledge all recovery warnings",
        &format!(
            "• {}\n\n• {}\n\n• {}",
            warnings.no_cloud_backup(),
            warnings.device_and_phrase_loss(),
            warnings.support_cannot_recover()
        ),
        "I acknowledge all three warnings",
    )
    .await
}

pub(super) async fn confirm_public_address(
    app: &tauri::AppHandle,
    title: &str,
    address: &str,
    warning: &str,
) -> Result<bool, IpcError> {
    confirm(
        app,
        title,
        &format!("Address\n{address}\n\n{warning}"),
        "Confirm exact account",
    )
    .await
}

pub(crate) async fn confirm(
    app: &tauri::AppHandle,
    title: &str,
    body: &str,
    accept: &str,
) -> Result<bool, IpcError> {
    let title = title.to_owned();
    let body = body.to_owned();
    let accept = accept.to_owned();
    let (sender, receiver) = oneshot::channel();
    app.run_on_main_thread(move || {
        let result = autoreleasepool(|_| {
            let mtm = MainThreadMarker::new().ok_or_else(IpcError::internal)?;
            let alert = NSAlert::new(mtm);
            alert.setAlertStyle(NSAlertStyle::Warning);
            alert.setMessageText(&NSString::from_str(&title));
            alert.setInformativeText(&NSString::from_str(&body));
            alert.addButtonWithTitle(&NSString::from_str(&accept));
            alert.addButtonWithTitle(&NSString::from_str("Cancel"));
            Ok(alert.runModal() == NSAlertFirstButtonReturn)
        });
        let _ = sender.send(result);
    })
    .map_err(|_| IpcError::internal())?;
    receiver.await.map_err(|_| IpcError::internal())?
}

/// Acknowledge a public operation result without introducing another decision.
pub(super) async fn show_message(
    app: &tauri::AppHandle,
    title: &str,
    body: &str,
) -> Result<(), IpcError> {
    let title = title.to_owned();
    let body = body.to_owned();
    let (sender, receiver) = oneshot::channel();
    app.run_on_main_thread(move || {
        let result = autoreleasepool(|_| {
            let mtm = MainThreadMarker::new().ok_or_else(IpcError::internal)?;
            let alert = NSAlert::new(mtm);
            alert.setAlertStyle(NSAlertStyle::Warning);
            alert.setMessageText(&NSString::from_str(&title));
            alert.setInformativeText(&NSString::from_str(&body));
            alert.addButtonWithTitle(&NSString::from_str("OK"));
            alert.runModal();
            alert.window().close();
            Ok(())
        });
        let _ = sender.send(result);
    })
    .map_err(|_| IpcError::internal())?;
    receiver.await.map_err(|_| IpcError::internal())?
}

pub(crate) async fn request_secret_owned(
    app: &tauri::AppHandle,
    title: &str,
    prompt: &str,
) -> Result<Option<Zeroizing<String>>, IpcError> {
    let title = title.to_owned();
    let prompt = prompt.to_owned();
    let (sender, receiver) = oneshot::channel();
    app.run_on_main_thread(move || {
        let result = autoreleasepool(|_| {
            let mtm = MainThreadMarker::new().ok_or_else(IpcError::internal)?;
            let alert = NSAlert::new(mtm);
            alert.setMessageText(&NSString::from_str(&title));
            alert.setInformativeText(&NSString::from_str(&prompt));
            alert.addButtonWithTitle(&NSString::from_str("Continue"));
            alert.addButtonWithTitle(&NSString::from_str("Cancel"));
            let field = NSSecureTextField::initWithFrame(
                mtm.alloc(),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(420.0, 24.0)),
            );
            alert.setAccessoryView(Some(&field));
            if alert.runModal() != NSAlertFirstButtonReturn {
                field.setStringValue(&NSString::new());
                alert.window().close();
                return Ok(None);
            }
            let text = field.stringValue();
            field.setStringValue(&NSString::new());
            alert.window().close();
            if text.length() > 4096 {
                return Err(IpcError::internal());
            }
            Ok(Some(Zeroizing::new(text.to_string())))
        });
        let _ = sender.send(result);
    })
    .map_err(|_| IpcError::internal())?;
    receiver.await.map_err(|_| IpcError::internal())?
}

/// Native dialog intent; creation guidance is shown only when choosing a password.
pub(crate) enum PasswordPrompt {
    Enter,
    Existing,
    Create,
    Confirm,
}

pub(crate) const PASSWORD_MISMATCH_MESSAGE: &str = "Passwords do not match; start again";

fn password_input_error(prompt: &PasswordPrompt, message: &str) -> IpcError {
    // The first entry is already policy-valid. An invalid confirmation cannot
    // equal it, so explain the mismatch rather than how to choose a password.
    super::wallet_error(
        match prompt {
            PasswordPrompt::Confirm => PASSWORD_MISMATCH_MESSAGE,
            PasswordPrompt::Enter | PasswordPrompt::Existing => {
                "Password wallet could not be authenticated"
            }
            PasswordPrompt::Create => message,
        },
        false,
    )
}

/// Captures a bounded credential directly into its wallet-owned zeroising type.
/// The oneshot also owns that type, so a cancelled receiver cannot discard an
/// unprotected String containing a password. Framework copies remain a limit.
pub(crate) async fn request_password(
    app: &tauri::AppHandle,
    prompt: PasswordPrompt,
) -> Result<Option<wallet_core::WalletPassword>, IpcError> {
    let (title, instructions) = match prompt {
        PasswordPrompt::Enter => ("Enter password", ""),
        PasswordPrompt::Existing => ("Enter existing password", ""),
        PasswordPrompt::Create => (
            "Enter new password",
            "Use at least 15 characters. Spaces and Unicode are allowed. Maximum 1024 UTF-8 bytes. A copied vault can be guessed offline; use a strong, unique passphrase. The password cannot recover lost keys; keep a separate backup for every recovery phrase and imported private key.",
        ),
        PasswordPrompt::Confirm => ("Confirm new password", "Enter the new password again."),
    };
    let (sender, receiver) = oneshot::channel();
    app.run_on_main_thread(move || {
        let result = autoreleasepool(|_| {
            let mtm = MainThreadMarker::new().ok_or_else(IpcError::internal)?;
            let alert = NSAlert::new(mtm);
            alert.setMessageText(&NSString::from_str(title));
            alert.setInformativeText(&NSString::from_str(instructions));
            alert.addButtonWithTitle(&NSString::from_str("Continue"));
            alert.addButtonWithTitle(&NSString::from_str("Cancel"));
            let field = NSSecureTextField::initWithFrame(
                mtm.alloc(),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(420.0, 24.0)),
            );
            alert.setAccessoryView(Some(&field));
            let accepted = alert.runModal() == NSAlertFirstButtonReturn;
            let text = field.stringValue();
            field.setStringValue(&NSString::new());
            alert.window().close();
            if !accepted {
                return Ok(None);
            }
            // Bound the native→UTF-8 copy before allocation; UTF-16 length is a
            // conservative small-copy guard, and WalletPassword checks exact bytes.
            if text.length() > wallet_core::MAX_WALLET_PASSWORD_BYTES {
                return Err(password_input_error(&prompt, "Password is too long"));
            }
            wallet_core::WalletPassword::new(text.to_string())
                .map(Some)
                .map_err(|e| password_input_error(&prompt, &e.to_string()))
        });
        let _ = sender.send(result);
    })
    .map_err(|_| IpcError::internal())?;
    receiver.await.map_err(|_| IpcError::internal())?
}

/// Visible ordinary field exclusively for an exact public destructive confirmation.
pub(super) async fn request_confirmation(
    app: &tauri::AppHandle,
    title: &str,
    required: &str,
    explanation: &str,
) -> Result<bool, IpcError> {
    let title = title.to_owned();
    let required = required.to_owned();
    let explanation = explanation.to_owned();
    let (sender, receiver) = oneshot::channel();
    app.run_on_main_thread(move || {
        let result = autoreleasepool(|_| {
            let mtm = MainThreadMarker::new().ok_or_else(IpcError::internal)?;
            let alert = NSAlert::new(mtm);
            alert.setAlertStyle(NSAlertStyle::Warning);
            alert.setMessageText(&NSString::from_str(&title));
            alert.setInformativeText(&NSString::from_str(&format!(
                "{explanation}\n\nType this entire line exactly, including the space:\n\n{required}"
            )));
            alert.addButtonWithTitle(&NSString::from_str("Continue"));
            alert.addButtonWithTitle(&NSString::from_str("Cancel"));
            let field = NSTextField::initWithFrame(
                mtm.alloc(),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(600.0, 24.0)),
            );
            alert.setAccessoryView(Some(&field));
            let accepted = alert.runModal() == NSAlertFirstButtonReturn;
            let text = field.stringValue();
            field.setStringValue(&NSString::new());
            alert.window().close();
            if !accepted {
                return Ok(false);
            }
            if text.length() > required.len() || text.to_string() != required {
                return Err(super::wallet_error(
                    "The confirmation did not match; type the entire line exactly",
                    false,
                ));
            }
            Ok(true)
        });
        let _ = sender.send(result);
    })
    .map_err(|_| IpcError::internal())?;
    receiver.await.map_err(|_| IpcError::internal())?
}
pub(crate) fn abort_prompt(app: &tauri::AppHandle) {
    let _ = app.run_on_main_thread(|| {
        if let Some(mtm) = MainThreadMarker::new() {
            NSApplication::sharedApplication(mtm).abortModal();
        }
    });
}

#[cfg(test)]
mod password_prompt_tests {
    use super::*;

    #[test]
    fn invalid_confirmation_reports_mismatch_while_new_password_keeps_policy_guidance()
    -> Result<(), Box<dyn std::error::Error>> {
        for input in [String::new(), "short".to_owned(), "x".repeat(1025)] {
            let failure = wallet_core::WalletPassword::new(input)
                .err()
                .ok_or("synthetic invalid input was accepted")?;
            assert_eq!(
                password_input_error(&PasswordPrompt::Confirm, &failure.to_string()).message,
                PASSWORD_MISMATCH_MESSAGE
            );
            assert_eq!(
                password_input_error(&PasswordPrompt::Create, &failure.to_string()).message,
                failure.to_string()
            );
        }
        assert_eq!(
            password_input_error(&PasswordPrompt::Confirm, "Password is too long").message,
            PASSWORD_MISMATCH_MESSAGE
        );
        Ok(())
    }
}
