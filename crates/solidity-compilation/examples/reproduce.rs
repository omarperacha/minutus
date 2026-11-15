//! Read-only source-reproduction probe. Build with the normal reviewed helper
//! manifest embedded in `__TEXT,__solc_worker` and copy that helper beside it.
use solidity_compilation::{CompilationInput, TrustedCompiler};
use std::{error::Error, fs, sync::atomic::AtomicBool};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 5 {
        return Err("expected compiler.js input.json compilerVersion file contractName".into());
    }
    let compiler = fs::read(&args[0])?;
    let standard = serde_json::from_slice(&fs::read(&args[1])?)?;
    let input = CompilationInput::new(&args[2], &args[3], &args[4], standard)?;
    let output =
        TrustedCompiler::packaged()?.reproduce(&input, &compiler, &AtomicBool::new(false))?;
    println!(
        "{}",
        serde_json::json!({"compiler":output.compiler_version(),
        "runtimeBytes":output.runtime().len(),"commitment":output.commitment(),
        "abi":output.abi(),"storageLayout":output.storage_layout(),"methods":output.methods()})
    );
    Ok(())
}
