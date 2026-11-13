//! macOS-owned wallet secret presentation and input surfaces.
mod password_prompt;
pub(crate) use password_prompt::{PASSWORD_MISMATCH_MESSAGE, PasswordPrompt};

use ipc_types::IpcError;
use objc2::{MainThreadMarker, rc::autoreleasepool};
#[cfg(feature = "password-vault-prototype")]
use objc2_app_kit::NSRunningApplication;
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertStyle, NSApplication, NSSecureTextField, NSTextField,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
#[cfg(feature = "password-vault-prototype")]
use std::{
    thread,
    time::{Duration, Instant},
};
use tokio::sync::oneshot;
use wallet_core::{RecoveryPhrase, RecoveryVerificationWarnings};
use zeroize::Zeroizing;

/// Reports whether Minutus remains the active macOS application.
///
/// Legacy prototype authorisation retains its foreground policy. A native
/// authentication window can temporarily remove focus from the webview
/// window without backgrounding Minutus, so window focus is not a sound proxy for
/// this security boundary.
#[cfg(feature = "password-vault-prototype")]
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

/// Captures a bounded credential directly into its wallet-owned zeroising type.
/// The oneshot also owns that type, so a cancelled receiver cannot discard an
/// unprotected String containing a password. Framework copies remain a limit.
pub(crate) async fn request_password(
    app: &tauri::AppHandle,
    prompt: PasswordPrompt,
) -> Result<Option<wallet_core::WalletPassword>, IpcError> {
    let presentation = prompt.presentation();
    let (sender, receiver) = oneshot::channel();
    app.run_on_main_thread(move || {
        let result = autoreleasepool(|_| {
            let mtm = MainThreadMarker::new().ok_or_else(IpcError::internal)?;
            // Request foreground presentation rather than rejecting work that
            // completed while another app was active. Explicit modal acceptance
            // and fresh password verification establish presence; focus after
            // that click is not authority and may already have changed again.
            activate_password_prompt(mtm);
            let alert = NSAlert::new(mtm);
            alert.setMessageText(&NSString::from_str(presentation.title));
            alert.setInformativeText(&NSString::from_str(presentation.instructions));
            alert.addButtonWithTitle(&NSString::from_str(presentation.accept));
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
                return Err(prompt.input_error("Password is too long"));
            }
            wallet_core::WalletPassword::new(text.to_string())
                .map(Some)
                .map_err(|e| prompt.input_error(&e.to_string()))
        });
        let _ = sender.send(result);
    })
    .map_err(|_| IpcError::internal())?;
    receiver.await.map_err(|_| IpcError::internal())?
}

#[allow(
    deprecated,
    reason = "The supported macOS 13.3 floor requires this AppKit activation API for explicit native credential presentation"
)]
fn activate_password_prompt(mtm: MainThreadMarker) {
    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
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
