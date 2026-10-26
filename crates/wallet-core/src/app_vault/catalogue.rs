use super::document::{AppVaultDocument, MAX_ENTRIES};
use crate::{
    AccountMetadata, EvmPrivateKeyImportCommit, EvmSigningKey, PasswordVaultError as Error,
    RecoverySeed, VaultDocument, VaultEncryptionKey, VaultKeyId, open_vault_signing_key,
    seal_imported_private_key_vault, seal_recovery_seed_vault,
};
use zeroize::Zeroizing;

/// Validated account material sealed inside the application catalogue. Never IPC.
pub struct AppVaultEntry {
    pub(super) id: VaultKeyId,
    pub(super) payload: VaultDocument,
    pub(super) key: VaultEncryptionKey,
}
impl AppVaultEntry {
    /// Creates an entry from a verified recovery seed using existing payload crypto.
    /// # Errors
    /// Rejects randomness, invalid metadata or a mismatched derived account.
    pub fn recovery(
        id: VaultKeyId,
        seed: &RecoverySeed,
        account: AccountMetadata,
    ) -> Result<Self, Error> {
        let key = VaultEncryptionKey::generate().map_err(|_| Error::RandomnessUnavailable)?;
        let payload = seal_recovery_seed_vault(&key, seed, vec![account])
            .map_err(|_| Error::InvalidDocument)?;
        Self::validate(id, payload, key)
    }
    /// Creates an entry only from the existing confirmed private-key import capability.
    /// # Errors
    /// Rejects randomness, invalid metadata or a mismatched imported account.
    pub fn imported(
        id: VaultKeyId,
        commit: EvmPrivateKeyImportCommit,
        account: AccountMetadata,
    ) -> Result<Self, Error> {
        let key = VaultEncryptionKey::generate().map_err(|_| Error::RandomnessUnavailable)?;
        let payload = seal_imported_private_key_vault(&key, commit, vec![account])
            .map_err(|_| Error::InvalidDocument)?;
        Self::validate(id, payload, key)
    }
    pub(super) fn validate(
        id: VaultKeyId,
        payload: VaultDocument,
        key: VaultEncryptionKey,
    ) -> Result<Self, Error> {
        open_vault_signing_key(&payload, key.expose_secret(), id.clone())
            .map_err(|_| Error::AuthenticationFailed)?;
        if payload.accounts().len() != 1 {
            return Err(Error::InvalidDocument);
        }
        Ok(Self { id, payload, key })
    }
    /// Public stable entry identity.
    #[must_use]
    pub const fn id(&self) -> &VaultKeyId {
        &self.id
    }
    /// Authenticated account metadata.
    #[must_use]
    pub fn account(&self) -> &AccountMetadata {
        &self.payload.accounts()[0]
    }
}

/// Authenticated catalogue and master key. No cloning, formatting or serialisation.
pub struct AppVaultOpen {
    pub(super) master: VaultEncryptionKey,
    pub(super) entries: Vec<AppVaultEntry>,
    pub(super) revision: [u8; 32],
    pub(super) id: VaultKeyId,
}
impl AppVaultOpen {
    /// Authenticated catalogue revision.
    #[must_use]
    pub const fn revision(&self) -> [u8; 32] {
        self.revision
    }
    /// Public catalogue entries; keys remain private to wallet-core.
    #[must_use]
    pub fn entries(&self) -> &[AppVaultEntry] {
        &self.entries
    }
    /// Whether these capabilities belong to this exact encrypted root.
    /// # Errors
    /// Returns revision serialisation errors.
    pub fn matches(&self, document: &AppVaultDocument) -> Result<bool, Error> {
        Ok(self.id == *document.id() && self.revision == document.revision()?)
    }
    /// Constructs the exact selected signer without changing global inactivity.
    /// # Errors
    /// Rejects unknown entries or inconsistent authenticated account material.
    pub fn signer(&self, id: &VaultKeyId) -> Result<EvmSigningKey, Error> {
        let entry = self.entry(id)?;
        open_vault_signing_key(&entry.payload, entry.key.expose_secret(), id.clone())
            .map_err(|_| Error::AuthenticationFailed)
    }
    pub(super) fn entry(&self, id: &VaultKeyId) -> Result<&AppVaultEntry, Error> {
        self.entries
            .iter()
            .find(|e| &e.id == id)
            .ok_or(Error::NotFound)
    }
    /// Adds a new entry without replacing any existing account.
    /// Does not persist; the caller must atomically commit the exact root revision.
    /// # Errors
    /// Rejects duplicate identities/addresses or excessive entries.
    pub fn add(&mut self, entry: AppVaultEntry) -> Result<(), Error> {
        if self.entries.len() >= MAX_ENTRIES
            || self
                .entries
                .iter()
                .any(|e| e.id == entry.id || e.account().address() == entry.account().address())
        {
            return Err(Error::Conflict);
        }
        self.entries.push(entry);
        self.entries
            .sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
        Ok(())
    }
    /// Removes the exact selected entry. An empty catalogue must be deleted.
    /// # Errors
    /// Rejects unknown entries.
    pub fn remove(&mut self, id: &VaultKeyId) -> Result<(), Error> {
        let index = self
            .entries
            .iter()
            .position(|e| &e.id == id)
            .ok_or(Error::NotFound)?;
        self.entries.remove(index);
        Ok(())
    }
    pub(super) fn encode_keys(&self) -> Result<Zeroizing<Vec<u8>>, Error> {
        if self.entries.is_empty() || self.entries.len() > MAX_ENTRIES {
            return Err(Error::InvalidDocument);
        }
        let mut bytes = Zeroizing::new(Vec::with_capacity(8192));
        bytes.push(1);
        bytes.extend_from_slice(
            &u16::try_from(self.entries.len())
                .map_err(|_| Error::InvalidDocument)?
                .to_be_bytes(),
        );
        for entry in &self.entries {
            bytes.push(u8::try_from(entry.id.as_str().len()).map_err(|_| Error::InvalidDocument)?);
            bytes.extend_from_slice(entry.id.as_str().as_bytes());
            bytes.extend_from_slice(entry.key.expose_secret());
        }
        Ok(bytes)
    }
}
