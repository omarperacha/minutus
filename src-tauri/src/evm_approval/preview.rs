//! Display-only debug fixture. This entry point precedes all application setup;
//! it cannot open storage/vaults, contact RPC, issue approval commitments or sign.
use super::presentation::ApprovalDisplay;
use ipc_types::IpcError;
use objc2::MainThreadMarker;
use objc2_app_kit::NSApplication;
use std::ffi::{OsStr, OsString};

pub(crate) fn run() -> Result<(), IpcError> {
    let mode = Mode::parse(std::env::args_os().skip(2))?;
    let display = display()?;
    if mode == Mode::Print {
        println!("{}", display.complete_body()?);
        return Ok(());
    }
    let main = MainThreadMarker::new().ok_or_else(IpcError::internal)?;
    let _application = NSApplication::sharedApplication(main);
    let decision = super::native::present(&display)?;
    println!(
        "Synthetic native display decision: {decision:?}; preview finished without application/vault/network setup"
    );
    Ok(())
}
fn display() -> Result<ApprovalDisplay, IpcError> {
    super::presentation::synthetic_preview()
}

#[derive(Debug, Eq, PartialEq)]
enum Mode {
    Modal,
    Print,
}
impl Mode {
    fn parse(mut arguments: impl Iterator<Item = OsString>) -> Result<Self, IpcError> {
        match (arguments.next(), arguments.next()) {
            (None, None) => Ok(Self::Modal),
            (Some(flag), None) if flag == OsStr::new("--print") => Ok(Self::Print),
            _ => Err(IpcError::invalid_request(
                "Expected --contract-approval-preview [--print]; this fixed display fixture accepts no other arguments",
            )),
        }
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn only_fixed_print_or_no_argument_modes_are_admitted() -> Result<(), String> {
        use super::Mode;
        assert_eq!(
            Mode::parse(std::iter::empty()).map_err(|error| error.message)?,
            Mode::Modal
        );
        assert_eq!(
            Mode::parse(["--print".into()].into_iter()).map_err(|error| error.message)?,
            Mode::Print
        );
        for arguments in [
            vec!["--unknown"],
            vec!["--print", "--print"],
            vec!["--print", "arbitrary text"],
            vec!["/private/path"],
        ] {
            assert!(Mode::parse(arguments.into_iter().map(Into::into)).is_err());
        }
        Ok(())
    }
    #[test]
    fn fixture_has_explicit_no_authority_and_scroll_end_marker() -> Result<(), String> {
        let display = super::display().map_err(|error| error.message)?;
        assert!(display.body.starts_with("SYNTHETIC DISPLAY FIXTURE"));
        let content = display.complete_body().map_err(|error| error.message)?;
        assert!(content.contains("END OF SYNTHETIC EVIDENCE"));
        assert!(content.lines().count() > 40);
        assert!(content.find("MODEL SUMMARY") < content.find("DISCLAIMER:"));
        assert!(display.approval_allowed);
        Ok(())
    }
}
