//! Packaging invariants for Zeta's local speech runtime.

use std::{error::Error, fs, path::Path};

use serde_json::Value;
use sha2::{Digest, Sha256};

#[test]
fn tauri_packages_the_speech_helper_and_runtime_policy() -> Result<(), Box<dyn Error>> {
    let manifest_directory = Path::new(env!("CARGO_MANIFEST_DIR"));
    let config: Value = serde_json::from_str(&fs::read_to_string(
        manifest_directory.join("tauri.conf.json"),
    )?)?;
    let bundle = &config["bundle"];
    assert!(bundle["externalBin"].as_array().is_some_and(|binaries| {
        binaries
            .iter()
            .any(|binary| binary == "binaries/zeta-whisper-helper")
    }));
    assert!(bundle["resources"].as_array().is_some_and(|resources| {
        resources
            .iter()
            .any(|resource| resource == "speech-runtime")
    }));
    assert_eq!(bundle["macOS"]["minimumSystemVersion"], "13.3");
    Ok(())
}

#[test]
fn runtime_inputs_permissions_and_notices_are_pinned() -> Result<(), Box<dyn Error>> {
    let manifest_directory = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repository = manifest_directory
        .parent()
        .ok_or("repository root is absent")?;
    let lock = fs::read_to_string(manifest_directory.join("speech-runtime/runtime.lock"))?;
    for required in [
        "runtime_revision=371b5a7561823ab2bb32142d2751e35e7534727b",
        "source_archive_sha256=59f16e008b0fe4f283b5bede96952aed5a8f2e16c2f2f721f263218982a153e7",
        "protocol_version=1",
        "model_sha256=394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
        "minimum_macos_version=13.3",
    ] {
        assert!(lock.lines().any(|line| line == required));
    }
    for (key, path) in [
        ("helper_source_sha256", "scripts/asr/helper.cpp"),
        ("cmake_source_sha256", "scripts/asr/CMakeLists.txt"),
        ("build_script_sha256", "scripts/asr/build_helper.py"),
        ("stage_script_sha256", "scripts/asr/stage_helper.py"),
        (
            "helper_entitlements_sha256",
            "src-tauri/WhisperHelper.entitlements.plist",
        ),
        ("app_info_sha256", "src-tauri/Info.plist"),
        (
            "whisper_cpp_license_sha256",
            "src-tauri/speech-runtime/WHISPER-CPP-LICENSE.txt",
        ),
        (
            "openai_whisper_license_sha256",
            "src-tauri/speech-runtime/OPENAI-WHISPER-LICENSE.txt",
        ),
    ] {
        let digest = format!("{:x}", Sha256::digest(fs::read(repository.join(path))?));
        assert!(
            lock.lines().any(|line| line == format!("{key}={digest}")),
            "missing digest for {path}"
        );
    }
    let info = fs::read_to_string(manifest_directory.join("Info.plist"))?;
    let app_entitlements = fs::read_to_string(manifest_directory.join("Entitlements.plist"))?;
    let helper_entitlements =
        fs::read_to_string(manifest_directory.join("WhisperHelper.entitlements.plist"))?;
    assert!(info.contains("NSMicrophoneUsageDescription"));
    assert!(app_entitlements.contains("com.apple.security.device.audio-input"));
    assert!(helper_entitlements.contains("com.apple.security.app-sandbox"));
    assert!(helper_entitlements.contains("com.apple.security.inherit"));
    assert!(!helper_entitlements.contains("com.apple.security.network"));
    Ok(())
}

#[test]
fn signing_and_verification_fail_closed_on_helper_policy() -> Result<(), Box<dyn Error>> {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("repository root is absent")?;
    let signing = fs::read_to_string(repository.join("scripts/sign-macos-bundle.sh"))?;
    let verification = fs::read_to_string(repository.join("scripts/verify-macos-bundle.sh"))?;
    for required in [
        "com.flowzed.zeta.whisper-helper",
        "WhisperHelper.entitlements.plist",
        "helper.signed.sha256",
    ] {
        assert!(signing.contains(required));
    }
    for required in [
        "com.flowzed.zeta.whisper-helper",
        "NSMicrophoneUsageDescription",
        "helper.signed.sha256",
        "system dynamic dependency",
        "entitlement set differs",
    ] {
        assert!(verification.contains(required));
    }
    Ok(())
}

#[test]
fn public_speech_contracts_remain_platform_neutral() -> Result<(), Box<dyn Error>> {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("repository root is absent")?;
    for contract in [
        repository.join("crates/model-runtime/src/asr/recognition.rs"),
        repository.join("crates/audio-runtime/src/contract.rs"),
        repository.join("crates/ipc-types/src/speech.rs"),
    ] {
        let source = fs::read_to_string(&contract)?;
        for platform_detail in ["CoreAudio", "AVFoundation", "Metal", "WASAPI", "macOS"] {
            assert!(
                !source.contains(platform_detail),
                "{} exposes {platform_detail}",
                contract.display()
            );
        }
    }
    Ok(())
}
