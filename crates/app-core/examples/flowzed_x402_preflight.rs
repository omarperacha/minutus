//! Explicitly unpaid, debug-only Flowzed emulator compatibility probe.

#[cfg(debug_assertions)]
#[path = "flowzed_preflight/mod.rs"]
mod preflight;

fn main() -> std::process::ExitCode {
    #[cfg(debug_assertions)]
    {
        match preflight::run() {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("Flowzed unpaid preflight failed: {error}");
                std::process::ExitCode::FAILURE
            }
        }
    }
    #[cfg(not(debug_assertions))]
    {
        eprintln!("Flowzed emulator preflight is disabled in release builds");
        std::process::ExitCode::FAILURE
    }
}
