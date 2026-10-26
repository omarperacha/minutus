//! Non-secret, non-mutating checks before collecting wallet recovery material.

use security_framework::{
    item::{CloudSync, ItemClass, ItemSearchOptions, Limit},
    os::macos::code_signing::{Flags, SecCode, SecRequirement},
};

use super::{VaultKeyStoreError, macos::map_keychain_error};

// A bundle identifier alone (including an ad-hoc codesign --identifier) does not
// grant Data Protection Keychain access. macOS must also accept the profile's
// application identity. The subsequent Keychain query checks OS enforcement.
const CALLER_REQUIREMENT: &str = r#"anchor apple generic
    and identifier "com.omarperacha.zeta"
    and entitlement["com.apple.application-identifier"] = *".com.omarperacha.zeta"
    and entitlement["com.apple.developer.team-identifier"] exists
    and entitlement["com.apple.security.app-sandbox"] exists
    and ! entitlement["keychain-access-groups"] exists"#;

pub(super) fn check_readiness() -> Result<(), VaultKeyStoreError> {
    let requirement: SecRequirement = CALLER_REQUIREMENT
        .parse()
        .map_err(|_| VaultKeyStoreError::PlatformFailure)?;
    SecCode::for_self(Flags::NONE)
        .and_then(|code| code.check_validity(Flags::NONE, &requirement))
        .map_err(|_| VaultKeyStoreError::SigningConfigurationUnavailable)?;

    // A dedicated service/account, never a wallet ID. Attributes only, no data
    // or references, no writes, and no authentication UI even if an item exists.
    let result = ItemSearchOptions::new()
        .class(ItemClass::generic_password())
        .service("com.flowzed.zeta.wallet.readiness")
        .account("non-secret-readiness-probe")
        .ignore_legacy_keychains()
        .cloud_sync(CloudSync::MatchSyncNo)
        .skip_authenticated_items(true)
        .load_data(false)
        .load_refs(false)
        .load_attributes(true)
        .limit(Limit::Max(1))
        .search()
        .map(|_| ())
        .map_err(map_keychain_error);
    readiness_result(result)
}

fn readiness_result(result: Result<(), VaultKeyStoreError>) -> Result<(), VaultKeyStoreError> {
    match result {
        Ok(()) | Err(VaultKeyStoreError::NotFound) => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caller_requirement_is_valid_security_framework_syntax()
    -> Result<(), security_framework::base::Error> {
        CALLER_REQUIREMENT.parse::<SecRequirement>()?;
        Ok(())
    }

    #[test]
    fn absent_probe_is_expected_but_platform_failures_are_not_readiness() {
        assert_eq!(readiness_result(Ok(())), Ok(()));
        assert_eq!(readiness_result(Err(VaultKeyStoreError::NotFound)), Ok(()));
        for error in [
            VaultKeyStoreError::SigningConfigurationUnavailable,
            VaultKeyStoreError::AuthorizationUnavailable,
            VaultKeyStoreError::AuthorizationDenied,
            VaultKeyStoreError::AuthorizationCancelled,
            VaultKeyStoreError::PlatformFailure,
        ] {
            assert_eq!(readiness_result(Err(error)), Err(error));
        }
    }
}
