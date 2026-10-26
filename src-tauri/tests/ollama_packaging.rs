//! Packaging invariants for Zeta's independent Glimmer and Qwen runtimes.

use std::{error::Error, fs, path::Path};

use serde_json::Value;
use sha2::{Digest, Sha256};

#[test]
fn hot_reload_keeps_the_existing_non_wallet_development_data_identity() -> Result<(), Box<dyn Error>>
{
    let manifest_directory = Path::new(env!("CARGO_MANIFEST_DIR"));
    let config: Value = serde_json::from_str(&fs::read_to_string(
        manifest_directory.join("tauri.dev.conf.json"),
    )?)?;
    assert_eq!(config["identifier"], "com.flowzed.zeta");
    Ok(())
}

#[test]
fn tauri_packages_managed_runtime_helpers() -> Result<(), Box<dyn Error>> {
    let manifest_directory = Path::new(env!("CARGO_MANIFEST_DIR"));
    let config: Value = serde_json::from_str(&fs::read_to_string(
        manifest_directory.join("tauri.conf.json"),
    )?)?;
    let bundle = &config["bundle"];
    assert_eq!(config["identifier"], "com.omarperacha.zeta");

    let external_bins = bundle["externalBin"]
        .as_array()
        .ok_or("externalBin must be an array")?;
    assert!(
        external_bins
            .iter()
            .any(|binary| binary == "binaries/zeta-tokenizer")
    );
    assert!(
        external_bins
            .iter()
            .any(|binary| binary == "binaries/zeta-computation-broker")
    );
    assert!(
        external_bins
            .iter()
            .any(|binary| binary == "binaries/zeta-computation-runner")
    );
    assert!(bundle["resources"].as_array().is_some_and(|resources| {
        resources
            .iter()
            .any(|resource| resource == "ollama-runtime")
    }));
    assert!(bundle["resources"].as_array().is_some_and(|resources| {
        resources
            .iter()
            .any(|resource| resource == "computation-service")
    }));
    Ok(())
}

#[test]
fn computation_xpc_service_has_a_separate_deny_by_default_sandbox() -> Result<(), Box<dyn Error>> {
    let manifest_directory = Path::new(env!("CARGO_MANIFEST_DIR"));
    let service =
        fs::read_to_string(manifest_directory.join("ComputationService.entitlements.plist"))?;

    assert!(service.contains("com.apple.security.app-sandbox"));
    for forbidden in [
        "com.apple.security.inherit",
        "com.apple.security.network.client",
        "com.apple.security.network.server",
        "com.apple.security.files.user-selected",
        "com.apple.security.files.bookmarks",
        "keychain-access-groups",
        "com.apple.security.application-groups",
    ] {
        assert!(!service.contains(forbidden));
    }
    for inherited_child in [
        "ComputationBroker.entitlements.plist",
        "ComputationRunner.entitlements.plist",
    ] {
        let entitlements = fs::read_to_string(manifest_directory.join(inherited_child))?;
        assert!(entitlements.contains("com.apple.security.app-sandbox"));
        assert!(entitlements.contains("com.apple.security.inherit"));
        assert!(!entitlements.contains("com.apple.security.network"));
    }
    Ok(())
}

#[test]
fn computation_runtime_and_policy_are_exactly_pinned() -> Result<(), Box<dyn Error>> {
    let manifest_directory = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repository = manifest_directory
        .parent()
        .ok_or("repository root is absent")?;
    let lock = fs::read_to_string(manifest_directory.join("computation-runtime.lock"))?;
    for exact in [
        "target=aarch64-apple-darwin",
        "transport=xpc-service-v1",
        "contract_version=zeta-contract-1",
        "runtime_version=quickjs-ng-0.15.1+rquickjs-0.12.2+zeta-contract-1",
        "rquickjs_version=0.12.2",
        "rquickjs_checksum=4e04e4eedfb060b503b5f0a2644abb890b0b3620d3fb674f9455f230014964e4",
        "rquickjs_sys_checksum=a13ac243b86a74120814ef7e9e30ad5a2c1199b7b9963b1cf7c84e4cdc1cad99",
        "runner_identifier=com.flowzed.zeta.computation-runner",
        "broker_identifier=com.flowzed.zeta.computation-broker",
        "service_identifier=com.flowzed.zeta.computation-service",
        "cpu_time_ms=3000",
        "wall_time_ms=5000",
        "process_memory_bytes=134217728",
        "javascript_heap_bytes=67108864",
        "javascript_stack_bytes=524288",
        "max_child_processes=1",
        "max_open_files=8",
    ] {
        assert!(lock.lines().any(|line| line == exact), "missing {exact}");
    }
    for (key, path) in [
        (
            "broker_entitlements_sha256",
            "ComputationBroker.entitlements.plist",
        ),
        (
            "service_entitlements_sha256",
            "ComputationService.entitlements.plist",
        ),
        (
            "runner_entitlements_sha256",
            "ComputationRunner.entitlements.plist",
        ),
        ("broker_source_sha256", "computation-xpc/broker.c"),
        ("service_source_sha256", "computation-xpc/service.c"),
        ("worker_source_sha256", "computation-xpc/worker.c"),
        ("worker_header_sha256", "computation-xpc/worker.h"),
        (
            "containment_probe_sha256",
            "computation-xpc/containment_probe.c",
        ),
        ("service_info_sha256", "computation-xpc/Info.plist"),
    ] {
        let digest = format!(
            "{:x}",
            Sha256::digest(fs::read(manifest_directory.join(path))?)
        );
        assert!(
            lock.lines().any(|line| line == format!("{key}={digest}")),
            "missing digest for {path}"
        );
    }
    let cargo_lock = fs::read_to_string(repository.join("Cargo.lock"))?;
    assert!(cargo_lock.contains(
        "name = \"rquickjs\"\nversion = \"0.12.2\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"4e04e4eedfb060b503b5f0a2644abb890b0b3620d3fb674f9455f230014964e4\""
    ));
    assert!(cargo_lock.contains(
        "name = \"rquickjs-sys\"\nversion = \"0.12.2\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"a13ac243b86a74120814ef7e9e30ad5a2c1199b7b9963b1cf7c84e4cdc1cad99\""
    ));
    Ok(())
}

#[test]
fn release_scripts_fail_closed_on_runner_integrity_signature_and_policy()
-> Result<(), Box<dyn Error>> {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("repository root is absent")?;
    let build_script = fs::read_to_string(repository.join("scripts/build-computation-runner.sh"))?;
    let bundle_build = fs::read_to_string(repository.join("src-tauri/build.rs"))?;
    let signing = fs::read_to_string(repository.join("scripts/sign-macos-bundle.sh"))?;
    let verification = fs::read_to_string(repository.join("scripts/verify-macos-bundle.sh"))?;

    for required in [
        "--locked",
        "lipo -archs",
        "DIGEST_MANIFEST",
        "SERVICE_MANIFEST",
        "otool -L",
        "-Werror",
    ] {
        assert!(build_script.contains(required));
    }
    for required in [
        "computation-runtime.lock",
        "transport=xpc-service-v1",
        "verify_computation_service",
        "verify_regular_digest",
        "a computation executable is not arm64-only",
    ] {
        assert!(bundle_build.contains(required));
    }
    assert!(signing.contains("com.flowzed.zeta.computation-runner"));
    assert!(signing.contains("com.flowzed.zeta.computation-service"));
    assert!(signing.contains("com.flowzed.zeta.computation-broker"));
    assert!(signing.contains("rm \"$RAW_RUNNER\""));
    assert!(signing.contains("computation-service.sha256"));
    for required in [
        "codesign --verify --deep --strict",
        "com.flowzed.zeta.computation-runner",
        "com.flowzed.zeta.computation-service",
        "com.flowzed.zeta.computation-broker",
        "computation-service.sha256",
        "unsafe raw computation child",
        "com.apple.security.inherit",
        "lipo -archs",
        "ZETA_RELEASE_VERIFICATION",
        "stapler validate",
        "EXPECTED_BUNDLE_ID=com.omarperacha.zeta ;;",
        "EXPECTED_BUNDLE_ID=com.omarperacha.zeta.staging ;;",
        "Identifier=$EXPECTED_BUNDLE_ID$",
        "Print :CFBundleIdentifier",
        "Release verification refuses the staging identity.",
        "private default Keychain access group",
    ] {
        assert!(
            verification.contains(required),
            "missing verification guard: {required}"
        );
    }
    Ok(())
}

#[test]
fn runtime_lock_pins_exact_official_payload_and_license() -> Result<(), Box<dyn Error>> {
    let runtime = Path::new(env!("CARGO_MANIFEST_DIR")).join("ollama-runtime");
    let lock = fs::read_to_string(runtime.join("runtime.lock"))?;
    let license = fs::read_to_string(runtime.join("OLLAMA-LICENSE"))?;

    for exact_entry in [
        "version=0.32.14",
        "source_revision=d67ad83426633195089509347ffd4fe795120198",
        "darwin_archive_bytes=153876009",
        "darwin_archive_sha256=c7e8b91485943785bc6d295d96551e971ec94c6829d0d6b3500366942dc50cd1",
    ] {
        assert!(lock.lines().any(|line| line == exact_entry));
    }
    assert!(license.starts_with("MIT License\n\nCopyright (c) Ollama"));
    Ok(())
}

#[test]
fn sandbox_explicitly_allows_the_loopback_child_listener() -> Result<(), Box<dyn Error>> {
    let entitlements =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Entitlements.plist"))?;

    assert!(entitlements.contains("com.apple.security.app-sandbox"));
    assert!(entitlements.contains("com.apple.security.network.client"));
    assert!(entitlements.contains("com.apple.security.network.server"));
    Ok(())
}
