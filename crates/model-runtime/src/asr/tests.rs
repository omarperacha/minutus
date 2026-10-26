use std::{error::Error, fs};

use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::*;

fn fixture() -> Result<(SpeechProfile, Vec<u8>), Box<dyn Error>> {
    let mut bytes = Vec::new();
    for value in [
        0x6767_6d6c_i32,
        51866,
        1500,
        1280,
        20,
        32,
        448,
        1280,
        20,
        4,
        128,
        2008,
    ] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(b"synthetic weights; never loaded by a recognizer");
    let mut doc: Value = serde_json::from_str(include_str!("../../catalogue/speech-v1.json"))?;
    doc["profiles"][0]["artifact"]["downloadBytes"] = bytes.len().into();
    doc["profiles"][0]["artifact"]["installedBytes"] = bytes.len().into();
    doc["profiles"][0]["artifact"]["sha256"] = format!("{:x}", Sha256::digest(&bytes)).into();
    let profile = crate::SpeechCatalogue::parse_json(&doc.to_string())?.profiles()[0].clone();
    Ok((profile, bytes))
}

fn retain(root: &Path, profile: &SpeechProfile, bytes: &[u8]) -> Result<(), Box<dyn Error>> {
    fs::write(
        root.join("partial")
            .join(format!("{}.part", profile.artifact().sha256())),
        bytes,
    )?;
    Ok(())
}

#[test]
fn verified_install_survives_restart_and_lease_excludes_removal() -> Result<(), Box<dyn Error>> {
    let (profile, bytes) = fixture()?;
    let root = TempDir::new()?;
    let store = SpeechModelStore::initialize(root.path())?;
    assert_eq!(store.status(&profile)?, SpeechModelStatus::Missing);
    assert!(!store.has_install_candidate(&profile)?);
    retain(root.path(), &profile, &bytes)?;
    let mut resumed = false;
    store.install(&profile, &DownloadCancellation::new(), |p| {
        resumed |= p.resumed();
    })?;
    assert!(resumed);
    drop(store);
    let store = SpeechModelStore::initialize(root.path())?;
    assert_eq!(store.status(&profile)?, SpeechModelStatus::Installed);
    assert!(store.has_install_candidate(&profile)?);
    let lease = store.acquire(&profile, &DownloadCancellation::new())?;
    assert_eq!(
        lease
            .artifact()
            .path()
            .extension()
            .and_then(|value| value.to_str()),
        Some("bin")
    );
    assert!(matches!(
        store.remove(&profile),
        Err(SpeechModelError::Busy)
    ));
    assert!(matches!(
        store.install(&profile, &DownloadCancellation::new(), |_| {}),
        Err(SpeechModelError::Busy)
    ));
    drop(lease);
    store.remove(&profile)?;
    assert_eq!(store.status(&profile)?, SpeechModelStatus::Missing);
    assert!(!store.has_install_candidate(&profile)?);
    assert!(
        store
            .acquire(&profile, &DownloadCancellation::new())
            .is_err()
    );
    Ok(())
}

#[test]
fn cancellation_during_verification_and_publication_remains_resumable() -> Result<(), Box<dyn Error>>
{
    let (profile, bytes) = fixture()?;
    for phase in [
        crate::DownloadPhase::Verifying,
        crate::DownloadPhase::Installing,
    ] {
        let root = TempDir::new()?;
        let store = SpeechModelStore::initialize(root.path())?;
        retain(root.path(), &profile, &bytes)?;
        let cancel = DownloadCancellation::new();
        let result = store.install(&profile, &cancel, |p| {
            if p.phase() == phase {
                cancel.cancel();
            }
        });
        assert!(matches!(
            result,
            Err(SpeechModelError::Download(DownloadError::Cancelled))
        ));
        assert_eq!(store.status(&profile)?, SpeechModelStatus::Missing);
        drop(store);
        let restarted = SpeechModelStore::initialize(root.path())?;
        restarted.install(&profile, &DownloadCancellation::new(), |_| {})?;
        assert_eq!(restarted.status(&profile)?, SpeechModelStatus::Installed);
    }
    Ok(())
}

#[test]
fn corruption_never_publishes_and_changed_installed_bytes_cannot_load() -> Result<(), Box<dyn Error>>
{
    let (profile, bytes) = fixture()?;
    let root = TempDir::new()?;
    let store = SpeechModelStore::initialize(root.path())?;
    let mut changed = bytes.clone();
    changed[48] ^= 1;
    retain(root.path(), &profile, &changed)?;
    assert!(matches!(
        store.install(&profile, &DownloadCancellation::new(), |_| {}),
        Err(SpeechModelError::Download(
            DownloadError::ChecksumMismatch { .. }
        ))
    ));
    assert_eq!(store.status(&profile)?, SpeechModelStatus::Missing);
    retain(root.path(), &profile, &bytes)?;
    store.install(&profile, &DownloadCancellation::new(), |_| {})?;
    let path = root
        .path()
        .join("installed")
        .join(format!("{}.bin", profile.artifact().sha256()));
    fs::write(&path, changed)?;
    assert_eq!(store.status(&profile)?, SpeechModelStatus::Incompatible);
    assert!(
        store
            .acquire(&profile, &DownloadCancellation::new())
            .is_err()
    );
    store.remove(&profile)?;
    assert_eq!(store.status(&profile)?, SpeechModelStatus::Missing);
    Ok(())
}

#[test]
fn correct_digest_with_wrong_header_is_not_compatible() -> Result<(), Box<dyn Error>> {
    let (profile, mut bytes) = fixture()?;
    bytes[0] ^= 1;
    let mut doc: Value = serde_json::from_str(include_str!("../../catalogue/speech-v1.json"))?;
    doc["profiles"][0]["artifact"]["downloadBytes"] = bytes.len().into();
    doc["profiles"][0]["artifact"]["installedBytes"] = bytes.len().into();
    doc["profiles"][0]["artifact"]["sha256"] = format!("{:x}", Sha256::digest(&bytes)).into();
    let wrong = crate::SpeechCatalogue::parse_json(&doc.to_string())?.profiles()[0].clone();
    let root = TempDir::new()?;
    let store = SpeechModelStore::initialize(root.path())?;
    retain(root.path(), &wrong, &bytes)?;
    assert!(matches!(
        store.install(&wrong, &DownloadCancellation::new(), |_| {}),
        Err(SpeechModelError::Download(
            DownloadError::IncompatibleArtifact
        ))
    ));
    assert_eq!(store.status(&wrong)?, SpeechModelStatus::Missing);
    doc["profiles"][0]["runtimeRevision"] = "unsupported".into();
    let incompatible = crate::SpeechCatalogue::parse_json(&doc.to_string())?.profiles()[0].clone();
    assert_eq!(
        store.status(&incompatible)?,
        SpeechModelStatus::Incompatible
    );
    assert!(
        store
            .install(&incompatible, &DownloadCancellation::new(), |_| {})
            .is_err()
    );
    assert!(
        store
            .acquire(&profile, &DownloadCancellation::new())
            .is_err()
    );
    Ok(())
}

#[test]
fn incomplete_partial_is_not_installed_and_removal_clears_it() -> Result<(), Box<dyn Error>> {
    let (profile, bytes) = fixture()?;
    let root = TempDir::new()?;
    let store = SpeechModelStore::initialize(root.path())?;
    retain(root.path(), &profile, &bytes[..20])?;
    assert_eq!(store.status(&profile)?, SpeechModelStatus::Missing);
    assert!(
        store
            .acquire(&profile, &DownloadCancellation::new())
            .is_err()
    );
    store.remove(&profile)?;
    assert_eq!(fs::read_dir(root.path().join("partial"))?.count(), 0);
    Ok(())
}
