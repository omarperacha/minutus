//! One-shot private computation runner process.

fn main() -> std::process::ExitCode {
    if computation_runtime::run_one_shot().is_ok() {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::from(1)
    }
}
