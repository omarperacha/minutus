//! Independent Node/OpenSSL Ed25519 fixtures; no release private key is retained.

use super::policy::{self, Release};
use base64::{Engine as _, engine::general_purpose::STANDARD};

const KEY: &str = include_str!("fixtures/public.txt");
const SIGNATURE: &str = include_str!("fixtures/signature.txt");
const ARTIFACT: &[u8] = include_bytes!("fixtures/installer.txt");

fn release() -> Release {
    Release {
        version: "0.2.0".to_owned(),
        url: "https://minutus.ai/releases/Minutus-0.2.0-arm64.dmg".to_owned(),
        signature: SIGNATURE.to_owned(),
    }
}

#[test]
fn independent_signature_accepts_exact_bytes_and_bound_version()
-> Result<(), Box<dyn std::error::Error>> {
    let key = policy::public_key(KEY).map_err(|()| "fixture key")?;
    let signature = policy::signature(&release()).map_err(|()| "fixture signature")?;
    let mut verifier = key.verify_stream(&signature)?;
    for chunk in ARTIFACT.chunks(7) {
        verifier.update(chunk);
    }
    verifier.finalize()?;
    assert!(policy::validate_signed_version(signature.trusted_comment(), "0.2.0").is_ok());
    assert!(policy::validate_signed_version(signature.trusted_comment(), "9.9.9").is_err());
    Ok(())
}

#[test]
fn signature_rejects_tampered_artifact_and_trusted_comment()
-> Result<(), Box<dyn std::error::Error>> {
    let key = policy::public_key(KEY).map_err(|()| "fixture key")?;
    let signature = policy::signature(&release()).map_err(|()| "fixture signature")?;
    assert!(
        key.verify(b"tampered installer", &signature, false)
            .is_err()
    );
    let mut modified = release();
    modified.signature = SIGNATURE.replace("version:0.2.0", "version:9.9.9");
    let modified = policy::signature(&modified).map_err(|()| "modified fixture")?;
    assert!(key.verify(ARTIFACT, &modified, false).is_err());
    Ok(())
}

#[test]
fn missing_key_signature_version_and_duplicate_version_fail_closed() {
    assert!(policy::public_key("").is_err());
    let mut missing = release();
    missing.signature.clear();
    assert!(policy::signature(&missing).is_err());
    for comment in [
        "timestamp:1",
        "version:0.2.0\tversion:0.2.0",
        "version:0.1.0",
    ] {
        assert!(policy::validate_signed_version(comment, "0.2.0").is_err());
    }
}

#[test]
fn embedded_key_is_valid_or_update_networking_stays_disabled()
-> Result<(), Box<dyn std::error::Error>> {
    if policy::PUBLIC_KEY.trim().is_empty() {
        assert!(super::download::check("0.1.0").is_err());
    } else {
        let key = policy::public_key(policy::PUBLIC_KEY).map_err(|()| "enrolled release key")?;
        let signature = policy::signature(&release()).map_err(|()| "fixture signature")?;
        // An enrolled release key must never accept the retained test signer.
        assert!(key.verify(ARTIFACT, &signature, false).is_err());
    }
    Ok(())
}

#[test]
fn foreign_key_and_unsigned_artifact_are_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let altered_key = KEY.replace("TEST ONLY", "FOREIGN KEY");
    // Keep the syntactic key ID but change its actual Ed25519 public key.
    let encoded = altered_key.lines().nth(1).ok_or("fixture")?;
    let mut bytes = STANDARD.decode(encoded)?;
    bytes[10] ^= 1;
    let other = minisign_verify::PublicKey::from_base64(&STANDARD.encode(bytes))?;
    let signature = policy::signature(&release()).map_err(|()| "fixture signature")?;
    assert!(other.verify(ARTIFACT, &signature, false).is_err());
    Ok(())
}

#[test]
fn rejects_rollback_and_untrusted_artifact_locations() {
    assert!(policy::validate_release(&release(), "0.1.0").is_ok());
    assert!(policy::validate_release(&release(), "0.2.0").is_err());
    assert!(policy::validate_release(&release(), "0.3.0").is_err());
    for url in [
        "http://minutus.ai/releases/update.dmg",
        "https://evil.test/releases/update.dmg",
        "https://minutus.ai:444/releases/update.dmg",
        "https://user@minutus.ai/releases/update.dmg",
        "https://minutus.ai/releases/update.dmg?token=x",
        "https://minutus.ai/other/update.dmg",
        "https://minutus.ai/releases/update.zip",
    ] {
        let mut invalid = release();
        invalid.url = url.to_owned();
        assert!(
            policy::validate_release(&invalid, "0.1.0").is_err(),
            "{url}"
        );
    }
}
