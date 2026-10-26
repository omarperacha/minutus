//! Reproduce App Sandbox traversal without touching an application container.
use super::*;

#[test]
fn storage_traverses_ancestors_without_directory_read_access()
-> Result<(), Box<dyn std::error::Error>> {
    const CHILD: &str = "MINUTUS_STORAGE_SANDBOX_FIXTURE";
    if let Some(path) = std::env::var_os(CHILD) {
        let ancestor = std::path::PathBuf::from(path);
        // Prove the child really lacks the read access used by the old walker.
        assert!(fs::open(&ancestor, FLAGS | OFlags::DIRECTORY, Mode::empty()).is_err());
        let path = ancestor.join("application-data").join("app-vault");
        let store = PasswordVaultStore::initialize(&path)?;
        assert!(store.vault_ids()?.is_empty());
        drop(store);
        assert!(
            PasswordVaultStore::initialize(&path)?
                .vault_ids()?
                .is_empty()
        );
        return Ok(());
    }
    let temp = tempfile::tempdir()?;
    let ancestor = temp.path().canonicalize()?;
    std::fs::create_dir(ancestor.join("application-data"))?;
    // The generated temp path contains no profile syntax. Deny only listing of
    // this ancestor; the actual application-data directory remains accessible.
    let literal = serde_json::to_string(ancestor.to_str().ok_or("non-UTF8 fixture")?)?;
    let profile = format!("(version 1)(allow default)(deny file-read-data (literal {literal}))");
    let output = std::process::Command::new("/usr/bin/sandbox-exec")
        .args(["-p", &profile])
        .arg(std::env::current_exe()?)
        .args([
            "--exact",
            "password_vault::storage::sandbox_tests::storage_traverses_ancestors_without_directory_read_access",
            "--nocapture",
        ])
        .env(CHILD, &ancestor)
        .output()?;
    assert!(
        output.status.success(),
        "sandbox child failed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}
