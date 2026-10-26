//! Boundary regression: normal onboarding validates private storage/native intent,
//! and never introduces a protected-Keychain/signing-identity prerequisite.
#[test]
fn native_onboarding_prepares_storage_and_password_before_recovery_input()
-> Result<(), Box<dyn std::error::Error>> {
    let raw = include_str!("../src/wallet_lifecycle/onboarding.rs");
    let compact: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
    let onboarding = compact.as_str();
    for (name, secret) in [
        ("create", "verified_seed(app"),
        ("restore", "native::request_secret_owned("),
        ("import", "imported_entry(app"),
    ] {
        let declaration = format!("pub(super)asyncfn{name}(");
        let body = onboarding
            .split_once(&declaration)
            .ok_or("missing onboarding flow")?
            .1;
        let prep = body
            .find("prepare(app,control).await?")
            .ok_or("missing native/storage preparation")?;
        let secret = body.find(secret).ok_or("missing secret operation")?;
        assert!(
            prep < secret,
            "{name} handles secrets before storage/password readiness"
        );
    }
    for source in [
        onboarding,
        include_str!("../src/wallet_lifecycle/control.rs"),
        include_str!("../src/wallet_lifecycle/signing.rs"),
    ] {
        assert!(!source.contains("MacOsVaultKeyStore"));
        assert!(!source.contains("check_readiness"));
        assert!(!source.contains("sign_evm_with_mac_os_user_presence"));
    }
    Ok(())
}
