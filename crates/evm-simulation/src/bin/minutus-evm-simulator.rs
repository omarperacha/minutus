//! Fixed one-shot local EVM worker; stdin/stdout contain only bounded protocol.
fn main() -> std::process::ExitCode {
    match evm_simulation::run_worker(std::io::stdin().lock(), std::io::stdout().lock()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("EVM worker stopped: {error:?}");
            std::process::ExitCode::FAILURE
        }
    }
}
