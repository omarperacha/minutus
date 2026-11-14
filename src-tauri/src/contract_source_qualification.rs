//! Early debug source qualification is separate from application startup.
#[cfg(debug_assertions)]
mod inspection;

pub(crate) fn dispatch() -> Option<std::process::ExitCode> {
    use std::{ffi::OsStr, process::ExitCode};
    if std::env::args_os().nth(1).as_deref() != Some(OsStr::new("--qualify-generic-source")) {
        return None;
    }
    #[cfg(debug_assertions)]
    let result = match inspection::run(&std::env::args_os().skip(2).collect::<Vec<_>>()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Read-only source qualification failed: {error}");
            ExitCode::FAILURE
        }
    };
    #[cfg(not(debug_assertions))]
    let result = {
        eprintln!("Generic source qualification requires a debug executable");
        ExitCode::FAILURE
    };
    Some(result)
}
