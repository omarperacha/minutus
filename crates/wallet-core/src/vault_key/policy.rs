pub(super) const KEYCHAIN_SERVICE: &str = "com.flowzed.zeta.wallet.vault-key";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct VaultKeychainPolicy {
    pub(super) synchronizes: bool,
    pub(super) protection: ProtectionPolicy,
    pub(super) authentication: AuthenticationPolicy,
    pub(super) access_group: AccessGroupPolicy,
    pub(super) data_protection_keychain: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProtectionPolicy {
    PasscodeSetThisDeviceOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AuthenticationPolicy {
    UserPresence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AccessGroupPolicy {
    CallingApplication,
}

pub(super) const POLICY: VaultKeychainPolicy = VaultKeychainPolicy {
    synchronizes: false,
    protection: ProtectionPolicy::PasscodeSetThisDeviceOnly,
    authentication: AuthenticationPolicy::UserPresence,
    access_group: AccessGroupPolicy::CallingApplication,
    data_protection_keychain: true,
};
