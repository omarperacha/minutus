//! macOS-owned modal transaction decision surface.

use ipc_types::{EvmApprovalDecision, IpcError};
use objc2::{MainThreadMarker, rc::autoreleasepool};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertSecondButtonReturn, NSAlertStyle,
    NSAutoresizingMaskOptions, NSScreen, NSScrollView, NSTextView,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use tokio::sync::oneshot;

use super::presentation::ApprovalDisplay;

pub(super) async fn show(
    app: &tauri::AppHandle,
    display: ApprovalDisplay,
) -> Result<EvmApprovalDecision, IpcError> {
    let (sender, receiver) = oneshot::channel();
    app.run_on_main_thread(move || {
        let decision = present(&display);
        let _ = sender.send(decision);
    })
    .map_err(|_| IpcError::internal())?;
    receiver.await.map_err(|_| IpcError::internal())?
}

/// Shared native renderer; a display alone carries no signing authority.
pub(super) fn present(display: &ApprovalDisplay) -> Result<EvmApprovalDecision, IpcError> {
    autoreleasepool(|_| {
        let mtm = MainThreadMarker::new().ok_or_else(IpcError::internal)?;
        let alert = NSAlert::new(mtm);
        alert.setAlertStyle(if display.has_warnings {
            NSAlertStyle::Warning
        } else {
            NSAlertStyle::Informational
        });
        alert.setMessageText(&NSString::from_str(&display.title));
        alert.setInformativeText(&NSString::from_str(
                "Review the network, account, recipient, amount, maximum fees and warnings below. Scroll for the complete transaction and technical details. Approval applies only to this exact transaction.",
            ));
        // Bound the review to the current screen. All trusted facts remain
        // selectable and scrollable; decision buttons stay outside the pane.
        let screen = NSScreen::mainScreen(mtm).map_or(NSSize::new(1024.0, 768.0), |screen| {
            screen.visibleFrame().size
        });
        let size = NSSize::new(
            (screen.width - 200.0).clamp(240.0, 680.0),
            (screen.height - 280.0).clamp(120.0, 360.0),
        );
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), size);
        let scroll = NSScrollView::initWithFrame(mtm.alloc(), frame);
        scroll.setHasVerticalScroller(true);
        let text_frame = NSRect::new(NSPoint::new(0.0, 0.0), scroll.contentSize());
        let view = NSTextView::initWithFrame(mtm.alloc(), text_frame);
        view.setRichText(false);
        view.setEditable(false);
        view.setSelectable(true);
        view.setVerticallyResizable(true);
        view.setHorizontallyResizable(false);
        view.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        view.setMinSize(NSSize::new(0.0, size.height));
        view.setMaxSize(NSSize::new(f64::MAX, f64::MAX));
        view.setString(&NSString::from_str(&display.body));
        scroll.setDocumentView(Some(&view));
        alert.setAccessoryView(Some(&scroll));

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
    })
}
