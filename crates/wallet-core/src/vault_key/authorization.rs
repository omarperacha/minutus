use super::{VaultEncryptionKey, VaultKeyId};

/// One-use proof that local user presence authorized a vault unlock.
///
/// Password authentication and the retained platform adapter issue this type
/// only inside wallet-core. It
/// intentionally implements neither cloning, debugging nor serialization.
pub struct VaultUnlockAuthorization {
    key_id: VaultKeyId,
    key: VaultEncryptionKey,
}

impl VaultUnlockAuthorization {
    pub(crate) fn new(key_id: VaultKeyId, key: VaultEncryptionKey) -> Self {
        Self { key_id, key }
    }

    pub(crate) fn into_parts(self) -> (VaultKeyId, VaultEncryptionKey) {
        (self.key_id, self.key)
    }
}

/// One-use proof that local user presence authorized a signing operation.
///
/// This capability is distinct from unlock authorization so an unlocked
/// session cannot mint signing authority. It intentionally implements neither
/// cloning, debugging nor serialization.
pub struct VaultSigningAuthorization {
    key_id: VaultKeyId,
    key: VaultEncryptionKey,
}

impl VaultSigningAuthorization {
    pub(crate) fn new(key_id: VaultKeyId, key: VaultEncryptionKey) -> Self {
        Self { key_id, key }
    }

    pub(crate) fn into_parts(self) -> (VaultKeyId, VaultEncryptionKey) {
        (self.key_id, self.key)
    }
}
