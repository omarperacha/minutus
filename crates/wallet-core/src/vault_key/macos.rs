use zeroize::Zeroizing;

use security_framework::{
    access_control::{ProtectionMode, SecAccessControl},
    passwords::{
        AccessControlOptions, PasswordOptions, delete_generic_password_options, generic_password,
        set_generic_password_options,
    },
};

use super::{
    VAULT_ENCRYPTION_KEY_BYTES, VaultEncryptionKey, VaultKeyId, VaultKeyStoreError,
    VaultSigningAuthorization, VaultUnlockAuthorization,
    policy::{AccessGroupPolicy, AuthenticationPolicy, KEYCHAIN_SERVICE, POLICY, ProtectionPolicy},
};

/// Device-only macOS Keychain storage for vault data-encryption keys.
///
/// Items use the Data Protection Keychain and the calling application's default
/// access group. The operating system therefore binds access to the signed
/// caller; no shared Keychain group is requested.
#[derive(Debug, Default, Clone, Copy)]
pub struct MacOsVaultKeyStore;

impl MacOsVaultKeyStore {
    /// Creates a store using the fixed reviewed Keychain policy.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Checks caller signing and protected Keychain access before onboarding.
    ///
    /// Does not read key bytes, write an item, or request user presence. Success
    /// is not authorization and does not guarantee a later key write or unlock.
    ///
    /// # Errors
    ///
    /// Returns a closed signing-configuration or Keychain-availability failure.
    pub fn check_readiness(&self) -> Result<(), VaultKeyStoreError> {
        super::macos_readiness::check_readiness()
    }

    /// Stores a vault key separately from its encrypted vault document.
    ///
    /// The item is device-only, explicitly non-synchronizing and protected by
    /// user presence. Calling this for an existing ID replaces only that ID's
    /// Keychain value; it never modifies vault-file material.
    ///
    /// # Errors
    ///
    /// Returns a closed platform or authorization failure without Keychain
    /// diagnostics or key bytes.
    pub fn store(
        &self,
        key_id: &VaultKeyId,
        key: &VaultEncryptionKey,
    ) -> Result<(), VaultKeyStoreError> {
        let options = password_options(key_id, true)?;
        set_generic_password_options(key.expose_secret(), options).map_err(map_keychain_error)
    }

    /// Loads the key after operating-system user-presence authorization.
    ///
    /// # Errors
    ///
    /// Returns a closed not-found, authorization, stored-data or platform
    /// failure. Retrieved temporary bytes are zeroized on every path.
    pub fn load_for_unlock(
        &self,
        key_id: &VaultKeyId,
    ) -> Result<VaultUnlockAuthorization, VaultKeyStoreError> {
        let key = load_user_present_key(key_id)?;
        Ok(VaultUnlockAuthorization::new(key_id.clone(), key))
    }

    /// Reloads the key with user presence immediately before signing.
    ///
    /// The access-control item lets macOS satisfy user presence with Touch ID
    /// when available or the device credential otherwise.
    ///
    /// # Errors
    ///
    /// Returns a closed not-found, authorization, stored-data or platform
    /// failure. Retrieved temporary bytes are zeroized on every path.
    pub fn load_for_signing(
        &self,
        key_id: &VaultKeyId,
    ) -> Result<VaultSigningAuthorization, VaultKeyStoreError> {
        let key = load_user_present_key(key_id)?;
        Ok(VaultSigningAuthorization::new(key_id.clone(), key))
    }

    /// Deletes only the separately stored Keychain key for this vault.
    ///
    /// # Errors
    ///
    /// Returns a closed authorization or platform failure. A missing key returns
    /// `Ok(false)`.
    pub fn delete(&self, key_id: &VaultKeyId) -> Result<bool, VaultKeyStoreError> {
        let options = password_options(key_id, false)?;
        match delete_generic_password_options(options) {
            Ok(()) => Ok(true),
            Err(error) if is_item_not_found(error) => Ok(false),
            Err(error) => Err(map_keychain_error(error)),
        }
    }
}

fn load_user_present_key(key_id: &VaultKeyId) -> Result<VaultEncryptionKey, VaultKeyStoreError> {
    let options = password_options(key_id, false)?;
    let bytes = Zeroizing::new(generic_password(options).map_err(map_keychain_error)?);
    let key_bytes: [u8; VAULT_ENCRYPTION_KEY_BYTES] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| VaultKeyStoreError::InvalidStoredKey)?;
    Ok(VaultEncryptionKey::from_bytes(key_bytes))
}

fn password_options(
    key_id: &VaultKeyId,
    include_access_control: bool,
) -> Result<PasswordOptions, VaultKeyStoreError> {
    let mut options = PasswordOptions::new_generic_password(KEYCHAIN_SERVICE, key_id.as_str());
    options.set_access_synchronized(Some(POLICY.synchronizes));
    if POLICY.data_protection_keychain {
        options.use_protected_keychain();
    }
    match POLICY.access_group {
        AccessGroupPolicy::CallingApplication => {}
    }
    if include_access_control {
        let protection = match POLICY.protection {
            ProtectionPolicy::PasscodeSetThisDeviceOnly => {
                ProtectionMode::AccessibleWhenPasscodeSetThisDeviceOnly
            }
        };
        let authentication = match POLICY.authentication {
            AuthenticationPolicy::UserPresence => AccessControlOptions::USER_PRESENCE,
        };
        let access_control =
            SecAccessControl::create_with_protection(Some(protection), authentication.bits())
                .map_err(|_| VaultKeyStoreError::PlatformFailure)?;
        options.set_access_control(access_control);
    }
    Ok(options)
}

pub(super) fn map_keychain_error(error: security_framework::base::Error) -> VaultKeyStoreError {
    use security_framework_sys::base::{errSecAuthFailed, errSecItemNotFound};

    // security-framework-sys does not expose these stable SecBase.h constants,
    // so keep the reviewed OSStatus values local.
    const ERR_SEC_USER_CANCELED: i32 = -128;
    const ERR_SEC_NOT_AVAILABLE: i32 = -25_291;
    const ERR_SEC_INTERACTION_NOT_ALLOWED: i32 = -25_308;
    const ERR_SEC_MISSING_ENTITLEMENT: i32 = -34_018;

    if error.code() == errSecItemNotFound {
        VaultKeyStoreError::NotFound
    } else if error.code() == ERR_SEC_MISSING_ENTITLEMENT {
        VaultKeyStoreError::SigningConfigurationUnavailable
    } else if error.code() == ERR_SEC_USER_CANCELED {
        VaultKeyStoreError::AuthorizationCancelled
    } else if error.code() == errSecAuthFailed {
        VaultKeyStoreError::AuthorizationDenied
    } else if matches!(
        error.code(),
        ERR_SEC_NOT_AVAILABLE | ERR_SEC_INTERACTION_NOT_ALLOWED
    ) {
        VaultKeyStoreError::AuthorizationUnavailable
    } else {
        VaultKeyStoreError::PlatformFailure
    }
}

fn is_item_not_found(error: security_framework::base::Error) -> bool {
    use security_framework_sys::base::errSecItemNotFound;

    error.code() == errSecItemNotFound
}

#[cfg(test)]
mod tests {
    use security_framework::base::Error;

    use super::*;

    #[test]
    fn owner_authorization_os_statuses_map_to_closed_outcomes() {
        assert_eq!(
            map_keychain_error(Error::from_code(-34_018)),
            VaultKeyStoreError::SigningConfigurationUnavailable
        );
        assert_eq!(
            map_keychain_error(Error::from_code(-25_300)),
            VaultKeyStoreError::NotFound
        );
        assert_eq!(
            map_keychain_error(Error::from_code(-1)),
            VaultKeyStoreError::PlatformFailure
        );
        assert_eq!(
            map_keychain_error(Error::from_code(-128)),
            VaultKeyStoreError::AuthorizationCancelled
        );
        assert_eq!(
            map_keychain_error(Error::from_code(-25_293)),
            VaultKeyStoreError::AuthorizationDenied
        );
        for unavailable in [-25_291, -25_308] {
            assert_eq!(
                map_keychain_error(Error::from_code(unavailable)),
                VaultKeyStoreError::AuthorizationUnavailable
            );
        }
    }
}
