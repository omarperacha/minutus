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
