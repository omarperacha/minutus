//! Early preparation diagnostics cannot enter normal wallet/application startup.
pub(crate) fn dispatch() -> Option<std::process::ExitCode> {
    use std::{ffi::OsStr, process::ExitCode};
    if std::env::args_os().nth(1).as_deref() != Some(OsStr::new("--qualify-contract-preparation")) {
        return None;
    }
    #[cfg(debug_assertions)]
    let result = match crate::evm_approval::contracts::qualify_preparation(
        &std::env::args_os().skip(2).collect::<Vec<_>>(),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Read-only contract preparation failed: {}", error.message);
            ExitCode::FAILURE
        }
    };
    #[cfg(not(debug_assertions))]
    let result = {
        eprintln!("Contract preparation qualification requires a debug executable");
        ExitCode::FAILURE
    };
    Some(result)
}
