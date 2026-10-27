//! Separate native metadata acknowledgement; no EVM approval is issued here.
use super::ReconciliationDisplay;
use crate::wallet_lifecycle::TransferCancellation;
use ipc_types::IpcError;
#[cfg(target_os = "macos")]
pub(super) async fn show(
    app: &tauri::AppHandle,
    display: ReconciliationDisplay,
    cancellation: TransferCancellation,
) -> Result<bool, IpcError> {
    use objc2::{MainThreadMarker, rc::autoreleasepool};
    use objc2_app_kit::{NSAlert, NSAlertSecondButtonReturn, NSAlertStyle};
    use objc2_foundation::NSString;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let result = autoreleasepool(|_| {
            cancellation.check()?;
            let mtm = MainThreadMarker::new().ok_or_else(IpcError::internal)?;
            let alert = NSAlert::new(mtm);
            alert.setAlertStyle(NSAlertStyle::Warning);
            alert.setMessageText(&NSString::from_str("Unresolved earlier transfer"));
            alert.setInformativeText(&NSString::from_str(&display.body));
            alert.addButtonWithTitle(&NSString::from_str("Keep blocked"));
            alert.addButtonWithTitle(&NSString::from_str("Allow new reviewed requests"));
            let allow = alert.runModal() == NSAlertSecondButtonReturn;
            cancellation.check()?;
            Ok(allow)
        });
        let _ = sender.send(result);
    })
    .map_err(|_| IpcError::internal())?;
    receiver.await.map_err(|_| IpcError::internal())?
}
#[cfg(not(target_os = "macos"))]
pub(super) async fn show(
    _app: &tauri::AppHandle,
    _display: ReconciliationDisplay,
    _cancellation: TransferCancellation,
) -> Result<bool, IpcError> {
    Err(IpcError::invalid_request(
        "Native transfer reconciliation requires macOS",
    ))
}
