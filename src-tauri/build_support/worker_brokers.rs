//! Main-image pins for role-specific sealed XPC launchers. Never linked into a worker.
use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub(crate) fn configure() -> Result<(), Box<dyn Error>> {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return Ok(());
    }
    let manifest = Path::new("worker-brokers/manifest.txt");
    for path in [
        "worker-brokers",
        "../crates/worker-transport/native",
        "../scripts/stage_worker_brokers.py",
        "../scripts/worker_broker_manifest.py",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
    if !manifest.exists() {
        if std::env::var("PROFILE").as_deref() == Ok("release") {
            return Err(
                "worker broker pins missing; stage the fixed XPC services before packaging".into(),
            );
        }
        return Ok(());
    }
    if !Command::new("python3")
        .args(["../scripts/stage_worker_brokers.py", "--verify-staged"])
        .status()?
        .success()
    {
        return Err("staged worker broker cohort verification failed".into());
    }
    let bytes = fs::read(manifest)?;
    if bytes.len() > 4096 || !bytes.starts_with(b"minutus-worker-brokers-v1\n") {
        return Err("invalid staged worker broker manifest".into());
    }
    let path = PathBuf::from(std::env::var_os("OUT_DIR").ok_or("missing OUT_DIR")?)
        .join("worker-brokers.manifest");
    fs::write(&path, bytes)?;
    let path = path
        .to_str()
        .filter(|path| !path.contains(','))
        .ok_or("unsupported broker manifest path")?;
    println!("cargo:rustc-link-arg=-Wl,-sectcreate,__TEXT,__worker_brokers,{path}");
    Ok(())
}
