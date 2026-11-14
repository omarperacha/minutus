//! Builds the private macOS XPC client bridge, without embedding runtime pins.
use std::{env, error::Error, path::PathBuf, process::Command};
fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=native/client.c");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return Ok(());
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").ok_or("Cargo omitted OUT_DIR")?);
    let object = out.join("client.o");
    let status = Command::new("xcrun")
        .args([
            "clang",
            "-arch",
            "arm64",
            "-mmacosx-version-min=13.3",
            "-std=c11",
            "-fblocks",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-c",
            "native/client.c",
            "-o",
        ])
        .arg(&object)
        .status()?;
    if !status.success() {
        return Err("Native worker client compilation failed".into());
    }
    let archive = out.join("libworker_transport_client.a");
    if !Command::new("xcrun")
        .args(["ar", "rcs"])
        .arg(&archive)
        .arg(&object)
        .status()?
        .success()
    {
        return Err("Native worker client archive failed".into());
    }
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=worker_transport_client");
    Ok(())
}
