//! macOS-owned modal transaction decision surface.

use ipc_types::{EvmApprovalDecision, IpcError};
use objc2::{MainThreadMarker, rc::autoreleasepool};
use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSAlertSecondButtonReturn, NSAlertStyle};
use objc2_foundation::NSString;
use tokio::sync::oneshot;

use super::presentation::ApprovalDisplay;

pub(super) async fn show(
    app: &tauri::AppHandle,
    display: ApprovalDisplay,
) -> Result<EvmApprovalDecision, IpcError> {
    let (sender, receiver) = oneshot::channel();
    app.run_on_main_thread(move || {
        let decision = autoreleasepool(|_| {
            let mtm = MainThreadMarker::new().ok_or_else(IpcError::internal)?;
            let alert = NSAlert::new(mtm);
            alert.setAlertStyle(if display.has_warnings {
                NSAlertStyle::Warning
            } else {
                NSAlertStyle::Informational
            });
            alert.setMessageText(&NSString::from_str(&display.title));
            alert.setInformativeText(&NSString::from_str(&display.body));

            // Denial is deliberately the default keyboard action. Approval
            // requires selecting the separate native button explicitly.
            alert.addButtonWithTitle(&NSString::from_str("Deny"));
            if display.approval_allowed {
                alert.addButtonWithTitle(&NSString::from_str("Approve exact transaction"));
            }
            alert.addButtonWithTitle(&NSString::from_str("Dismiss"));
            Ok(match alert.runModal() {
                response if response == NSAlertFirstButtonReturn => EvmApprovalDecision::Denied,
                response if response == NSAlertSecondButtonReturn && display.approval_allowed => {
                    EvmApprovalDecision::Approved
                }
                _ => EvmApprovalDecision::Dismissed,
            })
        });
        let _ = sender.send(decision);
    })
    .map_err(|_| IpcError::internal())?;
    receiver.await.map_err(|_| IpcError::internal())?
}
