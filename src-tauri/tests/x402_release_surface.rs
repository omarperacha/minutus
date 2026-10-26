//! Run only this integration target in release mode; it inspects the real binary.

#[test]
fn production_binary_advertises_x402_and_rejects_loopback_before_startup()
-> Result<(), Box<dyn std::error::Error>> {
    if cfg!(debug_assertions) {
        return Ok(());
    }
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_zeta-desktop"))
        .arg("--qualify-x402-release")
        // A production binary must ignore the debug configuration even when inherited.
        .env(
            "ZETA_X402_DEBUG_BASE_URL",
            "http://127.0.0.1:5001/qualification",
        )
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout)?;
    assert!(text.contains("public HTTPS/mainnet advertised"));
    assert!(text.contains("loopback rejected"));
    assert!(text.contains("no requests or payments executed"));
    Ok(())
}
