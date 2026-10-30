//! Embed exact staged helper identities in the loaded host image. Workers do not
//! contain this host-only manifest, avoiding a self-referential digest build.
use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    process::Command,
};
const NAMES: [&str; 2] = ["minutus-evm-simulator", "minutus-base-offline-simulator"];
pub(crate) fn configure() -> Result<(), Box<dyn Error>> {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return Ok(());
    }
    let target = std::env::var("TARGET")?;
    let manifest = Path::new("evm-workers/manifest.txt");
    println!("cargo:rerun-if-changed={}", manifest.display());
    for name in NAMES {
        println!("cargo:rerun-if-changed=binaries/{name}-{target}");
    }
    if !manifest.exists() {
        if std::env::var("PROFILE").as_deref() == Ok("release") {
            return Err(
                "EVM worker pins missing; run scripts/stage_evm_workers.py before packaging".into(),
            );
        }
        return Ok(()); // Raw developer builds cannot mint a trusted worker.
    }
    let text = fs::read_to_string(manifest)?;
    let mut lines = text.lines();
    if lines.next() != Some("minutus-evm-workers-v1") {
        return Err("invalid EVM worker manifest".into());
    }
    for name in NAMES {
        let fields: Vec<_> = lines
            .next()
            .ok_or("missing EVM worker pin")?
            .split(' ')
            .collect();
        if fields.len() != 3 || fields[0] != name || !hex(fields[1], 64) || !hex(fields[2], 40) {
            return Err("invalid EVM worker pin".into());
        }
        let path = Path::new("binaries").join(format!("{name}-{target}"));
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err("indirect EVM worker".into());
        }
        let digest = Command::new("/usr/bin/shasum")
            .args(["-a", "256"])
            .arg(&path)
            .output()?;
        if !digest.status.success()
            || String::from_utf8(digest.stdout)?.split_whitespace().next() != Some(fields[1])
        {
            return Err("EVM worker SHA256 mismatch".into());
        }
        let verified = Command::new("/usr/bin/codesign")
            .args(["--verify", "--strict"])
            .arg(&path)
            .output()?;
        if !verified.status.success() {
            return Err("invalid EVM worker code signature".into());
        }
        let signature = Command::new("/usr/bin/codesign")
            .args(["-d", "--verbose=4"])
            .arg(&path)
            .output()?;
        let details = String::from_utf8(signature.stderr)?;
        if !details.lines().any(|line| line == "Hash choices=sha256") {
            return Err("EVM worker must have a SHA256-only code directory".into());
        }
        if !signature.status.success()
            || !details
                .lines()
                .any(|line| line == format!("CDHash={}", fields[2]))
        {
            return Err("EVM worker CDHash mismatch".into());
        }
    }
    if lines.next().is_some() {
        return Err("unexpected EVM worker pins".into());
    }
    let embedded = PathBuf::from(std::env::var_os("OUT_DIR").ok_or("missing OUT_DIR")?)
        .join("evm-workers.manifest");
    fs::write(&embedded, text)?;
    let embedded = embedded
        .to_str()
        .filter(|path| !path.contains(','))
        .ok_or("unsupported EVM worker manifest path")?;
    println!("cargo:rustc-link-arg=-Wl,-sectcreate,__TEXT,__evm_workers,{embedded}");
    Ok(())
}
fn hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
