use super::{
    VaultEncryptionKey, VaultKeyId, VaultKeyStoreError,
    policy::{AccessGroupPolicy, AuthenticationPolicy, POLICY, ProtectionPolicy},
};

#[test]
fn key_ids_are_bounded_and_safe_for_exact_keychain_accounts() {
    assert!(VaultKeyId::new("wallet_01-primary").is_ok());
    for invalid in ["", "wallet/01", "wallet.01", "wallet 01"] {
        assert_eq!(
            VaultKeyId::new(invalid),
            Err(VaultKeyStoreError::InvalidKeyId)
        );
    }
    assert_eq!(
        VaultKeyId::new("a".repeat(65)),
        Err(VaultKeyStoreError::InvalidKeyId)
    );
}

#[test]
fn generated_vault_keys_have_the_required_width() -> Result<(), VaultKeyStoreError> {
    let key = VaultEncryptionKey::generate()?;

    assert_eq!(key.expose_secret().len(), 32);
    Ok(())
}

#[test]
fn keychain_policy_is_device_only_user_present_and_not_shared() {
    let policy = std::hint::black_box(POLICY);
    assert!(!policy.synchronizes);
    assert!(policy.data_protection_keychain);
    assert_eq!(
        policy.protection,
        ProtectionPolicy::PasscodeSetThisDeviceOnly
    );
    assert_eq!(policy.authentication, AuthenticationPolicy::UserPresence);
    assert_eq!(policy.access_group, AccessGroupPolicy::CallingApplication);
}
