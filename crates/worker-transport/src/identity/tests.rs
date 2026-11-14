use super::{parse_manifest, verify_file};
use crate::Role;
use sha2::{Digest, Sha256};
use std::{error::Error as StdError, fmt::Write, path::PathBuf};

fn manifest() -> Result<String, std::fmt::Error> {
    let mut text = "minutus-worker-brokers-v1\n".to_owned();
    for (role, worker) in [
        ("compiler", "minutus-solidity-compiler"),
        ("evm-online", "minutus-evm-simulator"),
        ("base-offline", "minutus-base-offline-simulator"),
    ] {
        writeln!(
            text,
            "{role} ai.minutus.worker.{role} minutus-{role}-broker {} {} {worker} {} {}",
            "a".repeat(64),
            "b".repeat(40),
            "c".repeat(64),
            "d".repeat(40)
        )?;
    }
    Ok(text)
}

#[test]
fn every_role_pin_is_required_even_when_another_role_is_selected() -> Result<(), Box<dyn StdError>>
{
    let text = manifest()?;
    for role in [Role::Compiler, Role::EvmOnline, Role::BaseOffline] {
        assert_eq!(parse_manifest(&text, role)?[0], role.name());
    }
    for altered in [
        text.replace("minutus-evm-online-broker", "../other"),
        text.replace("minutus-evm-simulator", "minutus-solidity-compiler"),
        text.replace(&"c".repeat(64), &"C".repeat(64)),
        text.replace("evm-online", "base-offline"),
        text.replace(
            " ai.minutus.worker.compiler",
            "  ai.minutus.worker.compiler",
        ),
        format!("{text}extra\n"),
        text.replace("minutus-worker-brokers-v1", "minutus-worker-brokers-v2"),
    ] {
        assert!(parse_manifest(&altered, Role::Compiler).is_err());
    }
    Ok(())
}

struct Directory(PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn file_pin_rejects_modified_bytes_and_indirect_final_component() -> Result<(), Box<dyn StdError>> {
    let mut random = [0_u8; 8];
    getrandom::fill(&mut random)?;
    let directory = Directory(std::env::temp_dir().join(format!(
        "minutus-transport-pin-{}",
        u64::from_ne_bytes(random)
    )));
    std::fs::create_dir(&directory.0)?;
    let file = directory.0.join("worker");
    std::fs::write(&file, b"reviewed worker")?;
    let digest = format!("{:x}", Sha256::digest(b"reviewed worker"));
    verify_file(&file, &digest)?;
    std::fs::write(&file, b"modified worker")?;
    assert!(verify_file(&file, &digest).is_err());
    #[cfg(unix)]
    {
        let link = directory.0.join("indirect");
        std::os::unix::fs::symlink(&file, &link)?;
        let changed = format!("{:x}", Sha256::digest(b"modified worker"));
        assert!(verify_file(&link, &changed).is_err());
    }
    assert!(verify_file(&directory.0, &digest).is_err());
    Ok(())
}
