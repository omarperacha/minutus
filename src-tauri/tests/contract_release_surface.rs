//! Qualifies the actual normal release binary without compiling unit fixtures.
#![cfg(not(debug_assertions))]

#[test]
fn production_binary_keeps_contract_workflows_disabled_before_startup()
-> Result<(), Box<dyn std::error::Error>> {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_zeta-desktop"))
        .arg("--qualify-contract-release")
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout)?;
    assert_eq!(
        text.trim(),
        "contract release surface: writes disabled; catalogue, startup and forecast schemas absent; no app setup, vault, RPC, native review or signing executed"
    );
    Ok(())
}

#[test]
fn production_binary_rejects_generic_qualification_before_startup()
-> Result<(), Box<dyn std::error::Error>> {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_zeta-desktop"))
        .arg("--qualify-generic-contract")
        .output()?;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr)?.trim(),
        "Generic contract qualification requires a debug executable"
    );
    Ok(())
}

#[test]
fn production_binary_rejects_source_qualification_before_native_settings()
-> Result<(), Box<dyn std::error::Error>> {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_zeta-desktop"))
        .arg("--qualify-generic-source")
        .output()?;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr)?.trim(),
        "Generic source qualification requires a debug executable"
    );
    Ok(())
}

#[test]
fn production_binary_rejects_workflow_preparation_before_native_settings()
-> Result<(), Box<dyn std::error::Error>> {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_zeta-desktop"))
        .arg("--qualify-contract-preparation")
        .output()?;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr)?.trim(),
        "Contract preparation qualification requires a debug executable"
    );
    Ok(())
}

#[test]
fn production_binary_rejects_cached_compiler_fixture_before_file_access()
-> Result<(), Box<dyn std::error::Error>> {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_zeta-desktop"))
        .args([
            "--qualify-contract-preparation",
            "--cached-compiler",
            "unopened-public-compiler-fixture.js",
        ])
        .output()?;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr)?.trim(),
        "Contract preparation qualification requires a debug executable"
    );
    Ok(())
}

#[test]
fn production_binary_rejects_stage_recovery_qualification_before_startup()
-> Result<(), Box<dyn std::error::Error>> {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_zeta-desktop"))
        .arg("--qualify-stage-argument-recovery")
        .output()?;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr)?.trim(),
        "Stage argument recovery qualification requires a debug executable"
    );
    Ok(())
}

#[test]
fn production_binary_rejects_review_print_before_native_ui_or_profile_setup()
-> Result<(), Box<dyn std::error::Error>> {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_zeta-desktop"))
        .args(["--contract-approval-preview", "--print"])
        .output()?;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr)?.trim(),
        "The synthetic native review preview is available only in macOS debug builds"
    );
    Ok(())
}
