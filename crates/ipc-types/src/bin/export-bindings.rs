//! Generates or checks the committed TypeScript IPC bindings.

use std::{env, error::Error, fs, io, path::PathBuf};

fn bindings_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../src/generated/ipc.ts")
}

fn run() -> Result<(), Box<dyn Error>> {
    let check = env::args().skip(1).any(|argument| argument == "--check");
    let path = bindings_path();
    let generated = ipc_types::typescript_bindings();

    if check {
        let existing = fs::read_to_string(&path).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "could not read {}: {error}; run pnpm bindings:generate",
                    path.display()
                ),
            )
        })?;
        if existing != generated {
            return Err(format!("{} is stale; run pnpm bindings:generate", path.display()).into());
        }
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, generated)?;
    println!("generated {}", path.display());
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("binding generation failed: {error}");
        std::process::exit(1);
    }
}
