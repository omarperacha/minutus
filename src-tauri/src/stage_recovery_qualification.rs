//! Fixed schema-routing diagnostics run before all normal application startup.
pub(crate) fn dispatch() -> Option<std::process::ExitCode> {
    use std::{ffi::OsStr, process::ExitCode};
    if std::env::args_os().nth(1).as_deref()
        != Some(OsStr::new("--qualify-stage-argument-recovery"))
    {
        return None;
    }
    #[cfg(debug_assertions)]
    let result = match crate::chat_runtime::qualify_stage_argument_recovery(
        &std::env::args_os().skip(2).collect::<Vec<_>>(),
    ) {
        Ok(text) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("Scripted stage argument recovery failed: {}", error.message);
            ExitCode::FAILURE
        }
    };
    #[cfg(not(debug_assertions))]
    let result = {
        eprintln!("Stage argument recovery qualification requires a debug executable");
        ExitCode::FAILURE
    };
    Some(result)
}
