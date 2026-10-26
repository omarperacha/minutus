use super::catalogue::{AppVaultEntry, AppVaultOpen};
use crate::{
    PasswordVaultError as Error, PasswordVaultJob, VaultDocument, VaultEncryptionKey, VaultKeyId,
    WalletPassword, password_vault::kdf,
};
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{AeadInPlace as _, KeyInit as _},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

pub(super) const MAX_ENTRIES: usize = 64;
pub(super) const MAX_BYTES: usize = 1024 * 1024;
const MAX_KEYS: usize = 8192;

/// Canonical encrypted-only app vault. Display metadata is untrusted until open.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppVaultDocument {
    wire: Wire,
    id: VaultKeyId,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Wire {
    version: u32,
    vault_id: String,
    kdf: Kdf,
    wrapping: Wrapping,
    catalogue: Catalogue,
    entries: Vec<Entry>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Kdf {
    algorithm: String,
    version: u32,
    #[serde(rename = "memoryKiB")]
    memory_kib: u32,
    iterations: u32,
    lanes: u32,
    salt: String,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Wrapping {
    algorithm: String,
    nonce: String,
    encrypted_key: String,
    tag: String,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Catalogue {
    nonce: String,
    encrypted_keys: String,
    tag: String,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Entry {
    entry_id: String,
    payload: String,
}

impl AppVaultDocument {
    /// Strict bounded versioned decoding, before any KDF work.
    /// # Errors
    /// Rejects noncanonical/unsupported documents, excessive counts and malformed fields.
    pub fn from_json(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_BYTES {
            return Err(Error::InvalidDocument);
        }
        let wire: Wire = serde_json::from_slice(bytes).map_err(|_| Error::InvalidDocument)?;
        let id = VaultKeyId::new(&wire.vault_id).map_err(|_| Error::InvalidDocument)?;
        let doc = Self { wire, id };
        doc.validate()?;
        if doc.to_json()? != bytes {
            return Err(Error::InvalidDocument);
        }
        Ok(doc)
    }
    /// Serialises encrypted-only material in canonical field order.
    /// # Errors
    /// Rejects format/size violations.
    pub fn to_json(&self) -> Result<Vec<u8>, Error> {
        let bytes = serde_json::to_vec(&self.wire).map_err(|_| Error::InvalidDocument)?;
        if bytes.len() > MAX_BYTES {
            return Err(Error::InvalidDocument);
        }
        Ok(bytes)
    }
    /// Stable application-vault identity.
    #[must_use]
    pub const fn id(&self) -> &VaultKeyId {
        &self.id
    }
    /// Exact canonical encrypted revision.
    /// # Errors
    /// Rejects invalid serialisation bounds.
    pub fn revision(&self) -> Result<[u8; 32], Error> {
        Ok(Sha256::digest(self.to_json()?).into())
    }
    /// Bounded public inventory; unauthenticated display facts never confer authority.
    /// # Errors
    /// Rejects invalid entry metadata.
    pub fn inventory(&self) -> Result<Vec<(VaultKeyId, crate::AccountMetadata)>, Error> {
        self.wire
            .entries
            .iter()
            .map(|e| {
                let payload = VaultDocument::from_json(e.payload.as_bytes())
                    .map_err(|_| Error::InvalidDocument)?;
                Ok((
                    VaultKeyId::new(&e.entry_id).map_err(|_| Error::InvalidDocument)?,
                    payload.accounts()[0].clone(),
                ))
            })
            .collect()
    }
    /// Creates the initial shared vault from a validated entry.
    /// # Errors
    /// Rejects cancelled work, randomness or crypto errors.
    pub fn create(
        id: VaultKeyId,
        entry: AppVaultEntry,
        password: &WalletPassword,
        job: &PasswordVaultJob,
    ) -> Result<Self, Error> {
        let open = AppVaultOpen {
            master: VaultEncryptionKey::generate().map_err(|_| Error::RandomnessUnavailable)?,
            entries: vec![entry],
            revision: [0; 32],
            id,
        };
        Self::seal(&open, password, job)
    }
    /// Reseals a freshly authenticated, mutated catalogue. No persistence implied.
    /// # Errors
    /// Rejects stale/cancelled work, duplicate metadata and crypto errors.
    pub fn seal(
        open: &AppVaultOpen,
        password: &WalletPassword,
        job: &PasswordVaultJob,
    ) -> Result<Self, Error> {
        job.check()?;
        let entries = open
            .entries
            .iter()
            .map(|e| {
                Ok(Entry {
                    entry_id: e.id.as_str().to_owned(),
                    payload: String::from_utf8(
                        e.payload.to_json().map_err(|_| Error::InvalidDocument)?,
                    )
                    .map_err(|_| Error::InvalidDocument)?,
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let nonce = random::<24>()?;
        let mut doc = Self {
            id: open.id.clone(),
            wire: Wire {
                version: 3,
                vault_id: open.id.as_str().to_owned(),
                kdf: profile([0; 32]),
                wrapping: Wrapping {
                    algorithm: "xchacha20-poly1305".to_owned(),
                    nonce: String::new(),
                    encrypted_key: String::new(),
                    tag: String::new(),
                },
                catalogue: Catalogue {
                    nonce: B64.encode(nonce),
                    encrypted_keys: String::new(),
                    tag: String::new(),
                },
                entries,
            },
        };
        let mut keys = open.encode_keys()?;
        let tag = cipher(open.master.expose_secret())?
            .encrypt_in_place_detached(
                XNonce::from_slice(&nonce),
                &doc.catalogue_aad()?,
                keys.as_mut(),
            )
            .map_err(|_| Error::CryptographyUnavailable)?;
        doc.wire.catalogue.encrypted_keys = B64.encode(&keys);
        doc.wire.catalogue.tag = B64.encode(tag);
        doc.wrap(&open.master, password, job)?;
        doc.validate()?;
        drop(doc.authenticate(password, job)?);
        Ok(doc)
    }
    /// Authenticates the app password and independently rederives every account.
    /// # Errors
    /// Returns closed authentication/cancellation/resource failures.
    pub fn authenticate(
        &self,
        password: &WalletPassword,
        job: &PasswordVaultJob,
    ) -> Result<AppVaultOpen, Error> {
        job.check()?;
        let derived = kdf::derive(password, &array::<32>(&self.wire.kdf.salt)?)?;
        job.check()?;
        let mut master = Zeroizing::new(array::<32>(&self.wire.wrapping.encrypted_key)?);
        let result = cipher(derived.as_ref())?.decrypt_in_place_detached(
            XNonce::from_slice(&array::<24>(&self.wire.wrapping.nonce)?),
            &self.wrap_aad()?,
            master.as_mut(),
            (&array::<16>(&self.wire.wrapping.tag)?).into(),
        );
        if result.is_err() {
            job.authentication_failed();
            return Err(Error::AuthenticationFailed);
        }
        let mut keys = Zeroizing::new(decode(&self.wire.catalogue.encrypted_keys, MAX_KEYS)?);
        cipher(master.as_ref())?
            .decrypt_in_place_detached(
                XNonce::from_slice(&array::<24>(&self.wire.catalogue.nonce)?),
                &self.catalogue_aad()?,
                keys.as_mut(),
                (&array::<16>(&self.wire.catalogue.tag)?).into(),
            )
            .map_err(|_| Error::AuthenticationFailed)?;
        let entries = self.decode_keys(&keys)?;
        job.check()?;
        job.authentication_succeeded();
        Ok(AppVaultOpen {
            master: VaultEncryptionKey::from_password_bytes(master),
            entries,
            revision: self.revision()?,
            id: self.id().clone(),
        })
    }
    /// Rewraps one shared master key; entries remain byte-identical.
    /// # Errors
    /// Rejects wrong passwords, stale work and crypto failures.
    pub fn change_password(
        &self,
        current: &WalletPassword,
        replacement: &WalletPassword,
        job: &PasswordVaultJob,
    ) -> Result<Self, Error> {
        let open = self.authenticate(current, job)?;
        let mut changed = self.clone();
        changed.wrap(&open.master, replacement, job)?;
        drop(changed.authenticate(replacement, job)?);
        Ok(changed)
    }
    fn wrap(
        &mut self,
        master: &VaultEncryptionKey,
        password: &WalletPassword,
        job: &PasswordVaultJob,
    ) -> Result<(), Error> {
        self.wire.kdf = profile(random::<32>()?);
        let nonce = random::<24>()?;
        self.wire.wrapping.nonce = B64.encode(nonce);
        job.check()?;
        let derived = kdf::derive(password, &array::<32>(&self.wire.kdf.salt)?)?;
        job.check()?;
        let mut bytes = Zeroizing::new(*master.expose_secret());
        let tag = cipher(derived.as_ref())?
            .encrypt_in_place_detached(
                XNonce::from_slice(&nonce),
                &self.wrap_aad()?,
                bytes.as_mut(),
            )
            .map_err(|_| Error::CryptographyUnavailable)?;
        self.wire.wrapping.encrypted_key = B64.encode(bytes.as_slice());
        self.wire.wrapping.tag = B64.encode(tag);
        job.check()
    }
    fn catalogue_aad(&self) -> Result<Vec<u8>, Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Aad<'a> {
            vault_id: &'a str,
            entries: &'a [Entry],
        }
        let mut bytes = b"minutus/app-vault/catalogue/v3\0".to_vec();
        bytes.extend(
            serde_json::to_vec(&Aad {
                vault_id: &self.wire.vault_id,
                entries: &self.wire.entries,
            })
            .map_err(|_| Error::InvalidDocument)?,
        );
        Ok(bytes)
    }
    fn wrap_aad(&self) -> Result<Vec<u8>, Error> {
        let mut wire = self.wire.clone();
        wire.wrapping.encrypted_key.clear();
        wire.wrapping.tag.clear();
        let mut bytes = b"minutus/app-vault/dek-wrap/v3\0".to_vec();
        bytes.extend(serde_json::to_vec(&wire).map_err(|_| Error::InvalidDocument)?);
        Ok(bytes)
    }
    fn decode_keys(&self, bytes: &[u8]) -> Result<Vec<AppVaultEntry>, Error> {
        if bytes.len() < 3
            || bytes[0] != 1
            || usize::from(u16::from_be_bytes([bytes[1], bytes[2]])) != self.wire.entries.len()
        {
            return Err(Error::AuthenticationFailed);
        }
        let mut cursor = 3;
        let mut entries = Vec::with_capacity(self.wire.entries.len());
        for entry in &self.wire.entries {
            let len = usize::from(*bytes.get(cursor).ok_or(Error::AuthenticationFailed)?);
            cursor += 1;
            let id = bytes
                .get(cursor..cursor + len)
                .ok_or(Error::AuthenticationFailed)?;
            cursor += len;
            if id != entry.entry_id.as_bytes() {
                return Err(Error::AuthenticationFailed);
            }
            let mut key = Zeroizing::new([0; 32]);
            key.copy_from_slice(
                bytes
                    .get(cursor..cursor + 32)
                    .ok_or(Error::AuthenticationFailed)?,
            );
            cursor += 32;
            entries.push(AppVaultEntry::validate(
                VaultKeyId::new(&entry.entry_id).map_err(|_| Error::AuthenticationFailed)?,
                VaultDocument::from_json(entry.payload.as_bytes())
                    .map_err(|_| Error::AuthenticationFailed)?,
                VaultEncryptionKey::from_password_bytes(key),
            )?);
        }
        if cursor != bytes.len() {
            return Err(Error::AuthenticationFailed);
        }
        Ok(entries)
    }
    fn validate(&self) -> Result<(), Error> {
        if self.wire.version != 3 {
            return Err(Error::UnsupportedVersion);
        }
        VaultKeyId::new(&self.wire.vault_id).map_err(|_| Error::InvalidDocument)?;
        if self.wire.kdf != profile(array::<32>(&self.wire.kdf.salt)?)
            || self.wire.wrapping.algorithm != "xchacha20-poly1305"
            || self.wire.entries.is_empty()
            || self.wire.entries.len() > MAX_ENTRIES
        {
            return Err(Error::InvalidDocument);
        }
        array::<24>(&self.wire.wrapping.nonce)?;
        array::<32>(&self.wire.wrapping.encrypted_key)?;
        array::<16>(&self.wire.wrapping.tag)?;
        array::<24>(&self.wire.catalogue.nonce)?;
        array::<16>(&self.wire.catalogue.tag)?;
        let keys = decode(&self.wire.catalogue.encrypted_keys, MAX_KEYS)?;
        if keys.len() < 3 {
            return Err(Error::InvalidDocument);
        }
        let mut previous = "";
        let mut addresses = std::collections::HashSet::new();
        for entry in &self.wire.entries {
            if entry.entry_id.as_str() <= previous || entry.payload.len() > 8192 {
                return Err(Error::InvalidDocument);
            }
            VaultKeyId::new(&entry.entry_id).map_err(|_| Error::InvalidDocument)?;
            let payload = VaultDocument::from_json(entry.payload.as_bytes())
                .map_err(|_| Error::InvalidDocument)?;
            if payload.accounts().len() != 1
                || payload.to_json().map_err(|_| Error::InvalidDocument)?
                    != entry.payload.as_bytes()
                || !addresses.insert(payload.accounts()[0].address().to_owned())
            {
                return Err(Error::InvalidDocument);
            }
            previous = &entry.entry_id;
        }
        Ok(())
    }
}
fn profile(salt: [u8; 32]) -> Kdf {
    Kdf {
        algorithm: "argon2id".to_owned(),
        version: 19,
        memory_kib: kdf::MEMORY_KIB,
        iterations: kdf::ITERATIONS,
        lanes: kdf::LANES,
        salt: B64.encode(salt),
    }
}
fn cipher(key: &[u8]) -> Result<XChaCha20Poly1305, Error> {
    XChaCha20Poly1305::new_from_slice(key).map_err(|_| Error::CryptographyUnavailable)
}
fn random<const N: usize>() -> Result<[u8; N], Error> {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).map_err(|_| Error::RandomnessUnavailable)?;
    Ok(bytes)
}
fn decode(value: &str, max: usize) -> Result<Vec<u8>, Error> {
    if value.len() > max.div_ceil(3) * 4 {
        return Err(Error::InvalidDocument);
    }
    let bytes = B64.decode(value).map_err(|_| Error::InvalidDocument)?;
    if bytes.len() > max || B64.encode(&bytes) != value {
        return Err(Error::InvalidDocument);
    }
    Ok(bytes)
}
fn array<const N: usize>(value: &str) -> Result<[u8; N], Error> {
    decode(value, N)?
        .try_into()
        .map_err(|_| Error::InvalidDocument)
}
