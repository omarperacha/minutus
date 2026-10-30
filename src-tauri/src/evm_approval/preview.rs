//! Display-only debug fixture. This entry point precedes all application setup;
//! it cannot open storage/vaults, contact RPC, issue approval commitments or sign.
use super::presentation::ApprovalDisplay;
use ipc_types::IpcError;
use objc2::MainThreadMarker;
use objc2_app_kit::NSApplication;

pub(crate) fn run() -> Result<(), IpcError> {
    let main = MainThreadMarker::new().ok_or_else(IpcError::internal)?;
    let _application = NSApplication::sharedApplication(main);
    let decision = super::native::present(&display())?;
    println!(
        "Synthetic native display decision: {decision:?}; preview finished without application/vault/network setup"
    );
    Ok(())
}
fn display() -> ApprovalDisplay {
    super::presentation::synthetic_preview()
}
#[cfg(test)]
mod tests {
    #[test]
    fn fixture_has_explicit_no_authority_and_scroll_end_marker() {
        let display = super::display();
        assert!(display.body.starts_with("SYNTHETIC DISPLAY FIXTURE"));
        assert!(display.body.contains("END OF SYNTHETIC EVIDENCE"));
        assert!(display.body.lines().count() > 40);
        assert!(display.approval_allowed);
    }
}
