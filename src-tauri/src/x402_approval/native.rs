//! macOS-owned modal x402 payment decision surface.

use ipc_types::{IpcError, X402ApprovalDecision};
use objc2::{MainThreadMarker, rc::autoreleasepool};
use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSAlertSecondButtonReturn, NSAlertStyle};
use objc2_foundation::NSString;
use tokio::sync::oneshot;

pub(super) async fn show(
    app: &tauri::AppHandle,
    title: String,
    body: String,
) -> Result<X402ApprovalDecision, IpcError> {
    let (sender, receiver) = oneshot::channel();
    app.run_on_main_thread(move || {
        let decision = autoreleasepool(|_| {
            let mtm = MainThreadMarker::new().ok_or_else(IpcError::internal)?;
            let alert = NSAlert::new(mtm);
            alert.setAlertStyle(NSAlertStyle::Warning);
            alert.setMessageText(&NSString::from_str(&title));
            alert.setInformativeText(&NSString::from_str(&body));
            // Deny remains the default keyboard action. Neither Return nor a
            // frontend value can select the separate exact-payment control.
            alert.addButtonWithTitle(&NSString::from_str("Deny"));
            alert.addButtonWithTitle(&NSString::from_str("Approve exact payment"));
            alert.addButtonWithTitle(&NSString::from_str("Dismiss"));
            Ok(match alert.runModal() {
                response if response == NSAlertFirstButtonReturn => X402ApprovalDecision::Denied,
                response if response == NSAlertSecondButtonReturn => X402ApprovalDecision::Approved,
                _ => X402ApprovalDecision::Dismissed,
            })
        });
        let _ = sender.send(decision);
    })
    .map_err(|_| IpcError::internal())?;
    receiver.await.map_err(|_| IpcError::internal())?
}
