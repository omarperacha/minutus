use ipc_types::IpcError;
use objc2::{MainThreadMarker, rc::autoreleasepool};
use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSAlertStyle, NSApplication, NSTextField};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use tokio::sync::oneshot;
use zeroize::Zeroizing;

#[derive(Clone, Copy)]
pub(super) enum Action {
    Create,
    Restore,
    Unlock,
    ChangePassword,
    Sign,
    Lock,
    Delete,
    Cancel,
}

pub(super) async fn choose(app: &tauri::AppHandle, summary: String) -> Result<Action, IpcError> {
    let (sender, receiver) = oneshot::channel();
    app.run_on_main_thread(move || {
        let result = autoreleasepool(|_| {
            let mtm = MainThreadMarker::new().ok_or_else(IpcError::internal)?;
            let alert = NSAlert::new(mtm);
            alert.setMessageText(&NSString::from_str("Password wallet prototype · test only"));
            let body = format!("{summary}\n\nUse fresh, unfunded test wallets only. Signing verifies local examples; nothing is broadcast. Existing wallets are untouched.");
            alert.setInformativeText(&NSString::from_str(&body));
            for title in [
                "Create test wallet", "Restore from recovery phrase", "Unlock", "Change password",
                "Sign synthetic examples", "Lock / cancel work",
                "Delete test wallet", "Cancel",
            ] {
                alert.addButtonWithTitle(&NSString::from_str(title));
            }
            let action = match alert.runModal() - NSAlertFirstButtonReturn {
                0 => Action::Create,
                1 => Action::Restore,
                2 => Action::Unlock,
                3 => Action::ChangePassword,
                4 => Action::Sign,
                5 => Action::Lock,
                6 => Action::Delete,
                _ => Action::Cancel,
            };
            alert.window().close();
            Ok(action)
        });
        let _ = sender.send(result);
    }).map_err(|_| IpcError::internal())?;
    receiver.await.map_err(|_| IpcError::internal())?
}

pub(super) async fn notice(app: &tauri::AppHandle, message: String) {
    let (sender, receiver) = oneshot::channel();
    let _ = app.run_on_main_thread(move || {
        autoreleasepool(|_| {
            if let Some(mtm) = MainThreadMarker::new() {
                let alert = NSAlert::new(mtm);
                alert.setMessageText(&NSString::from_str("Password wallet test"));
                alert.setInformativeText(&NSString::from_str(&message));
                alert.addButtonWithTitle(&NSString::from_str("OK"));
                let _ = alert.runModal();
                alert.window().close();
            }
        });
        let _ = sender.send(());
    });
    let _ = receiver.await;
}

/// Visible input for the public, exact wallet-deletion confirmation line.
pub(super) async fn request_deletion_confirmation(
    app: &tauri::AppHandle,
    required: &str,
) -> Result<Option<Zeroizing<String>>, IpcError> {
    let required = required.to_owned();
    let (sender, receiver) = oneshot::channel();
    app.run_on_main_thread(move || {
        let result = autoreleasepool(|_| {
            let mtm = MainThreadMarker::new().ok_or_else(IpcError::internal)?;
            let alert = NSAlert::new(mtm);
            alert.setAlertStyle(NSAlertStyle::Warning);
            alert.setMessageText(&NSString::from_str("Confirm test wallet deletion"));
            let instructions = format!(
                "Type the entire line below exactly, including uppercase DELETE and the space:\n\n{required}\n\nYou will enter your wallet password in the next step."
            );
            alert.setInformativeText(&NSString::from_str(&instructions));
            alert.addButtonWithTitle(&NSString::from_str("Continue to password"));
            alert.addButtonWithTitle(&NSString::from_str("Cancel"));
            let field = NSTextField::initWithFrame(
                mtm.alloc(),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(600.0, 24.0)),
            );
            field.setPlaceholderString(Some(&NSString::from_str("Type the full confirmation line")));
            alert.setAccessoryView(Some(&field));
            let accepted = alert.runModal() == NSAlertFirstButtonReturn;
            let text = field.stringValue();
            field.setStringValue(&NSString::new());
            alert.window().close();
            if !accepted {
                return Ok(None);
            }
            // Bound the native copy; wallet-core still enforces the exact,
            // printable-ASCII confirmation without trimming or normalisation.
            if text.length() > required.len() {
                return Err(crate::wallet_lifecycle::wallet_error(
                    "Type the confirmation line exactly as shown",
                    false,
                ));
            }
            Ok(Some(Zeroizing::new(text.to_string())))
        });
        let _ = sender.send(result);
    })
    .map_err(|_| IpcError::internal())?;
    receiver.await.map_err(|_| IpcError::internal())?
}

pub(super) fn abort_prompt(app: &tauri::AppHandle) {
    let _ = app.run_on_main_thread(|| {
        if let Some(mtm) = MainThreadMarker::new() {
            NSApplication::sharedApplication(mtm).abortModal();
        }
    });
}
