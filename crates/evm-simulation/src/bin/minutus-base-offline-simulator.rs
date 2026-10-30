//! Fixed signed-envelope worker. Raw protocol material never enters diagnostics.
fn main() -> std::process::ExitCode {
    match evm_simulation::offline::run_offline_worker(
        std::io::stdin().lock(),
        std::io::stdout().lock(),
    ) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(_) => std::process::ExitCode::FAILURE,
    }
}
