use super::{PasswordVaultError, kdf};
use crate::{VaultDocument, VaultKeyId};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// Password-wrapper version. Embedded payloads retain their independent v1 contract.
pub const PASSWORD_VAULT_VERSION: u32 = 2;
/// Maximum complete password envelope, enforced before parsing or KDF work.
pub const MAX_PASSWORD_VAULT_BYTES: usize = 32 * 1024;
const WRAPPING_ALGORITHM: &str = "xchacha20-poly1305";

/// Validated encrypted-only document. Public metadata remains untrusted until open.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PasswordVaultDocument {
    pub(super) id: VaultKeyId,
    pub(super) salt: [u8; 32],
    pub(super) nonce: [u8; 24],
    pub(super) encrypted_key: [u8; 32],
    pub(super) tag: [u8; 16],
    pub(super) payload: VaultDocument,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Wire {
    version: u32,
    vault_id: String,
    kdf: Kdf,
    wrapping: Wrapping,
    payload: String,
}
#[derive(Deserialize)]
struct Version {
    version: u32,
}
#[derive(Deserialize, Serialize)]
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
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Wrapping {
    algorithm: String,
    nonce: String,
    encrypted_key: String,
    tag: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Metadata<'a> {
    version: u32,
    vault_id: &'a str,
    kdf: Kdf,
    wrapping_algorithm: &'a str,
    nonce: String,
    payload_sha256: [u8; 32],
}

impl PasswordVaultDocument {
    /// Validates a bounded closed envelope before any password work.
    ///
    /// # Errors
    /// Rejects unknown/old versions, all unreviewed KDF parameters, unknown fields,
    /// noncanonical binary encodings and oversized or invalid embedded payloads.
    pub fn from_json(bytes: &[u8]) -> Result<Self, PasswordVaultError> {
        if bytes.len() > MAX_PASSWORD_VAULT_BYTES {
            return Err(PasswordVaultError::InvalidDocument);
        }
        // Typed version dispatch does not contact Keychain or interpret an old
        // document as permission to replace/create a wallet.
        let version: Version =
            serde_json::from_slice(bytes).map_err(|_| PasswordVaultError::InvalidDocument)?;
        if version.version != PASSWORD_VAULT_VERSION {
            return Err(PasswordVaultError::UnsupportedVersion);
        }
        let wire: Wire =
            serde_json::from_slice(bytes).map_err(|_| PasswordVaultError::InvalidDocument)?;
        if wire.kdf.algorithm != "argon2id"
            || wire.kdf.version != 19
            || wire.kdf.memory_kib != kdf::MEMORY_KIB
            || wire.kdf.iterations != kdf::ITERATIONS
            || wire.kdf.lanes != kdf::LANES
            || wire.wrapping.algorithm != WRAPPING_ALGORITHM
        {
            return Err(PasswordVaultError::InvalidDocument);
        }
        if wire.payload.len() > 8192 {
            return Err(PasswordVaultError::InvalidDocument);
        }
        let payload = VaultDocument::from_json(wire.payload.as_bytes())
            .map_err(|_| PasswordVaultError::InvalidDocument)?;
        if payload.accounts().len() != 1 {
            return Err(PasswordVaultError::InvalidDocument);
        }
        let result = Self {
            id: VaultKeyId::new(wire.vault_id).map_err(|_| PasswordVaultError::InvalidDocument)?,
            salt: decode(&wire.kdf.salt)?,
            nonce: decode(&wire.wrapping.nonce)?,
            encrypted_key: decode(&wire.wrapping.encrypted_key)?,
            tag: decode(&wire.wrapping.tag)?,
            payload,
        };
        // Storage and AAD have one canonical representation, including nested
        // payload JSON. Duplicate fields are rejected by the typed deserialisers.
        if result.to_json()? != bytes {
            return Err(PasswordVaultError::InvalidDocument);
        }
        Ok(result)
    }

    /// Serialises encrypted material only, using the fixed v2 schema.
    ///
    /// # Errors
    /// Returns a closed error if the bounded representation cannot be produced.
    pub fn to_json(&self) -> Result<Vec<u8>, PasswordVaultError> {
        if self.payload.accounts().len() != 1 {
            return Err(PasswordVaultError::InvalidDocument);
        }
        let payload = String::from_utf8(
            self.payload
                .to_json()
                .map_err(|_| PasswordVaultError::InvalidDocument)?,
        )
        .map_err(|_| PasswordVaultError::InvalidDocument)?;
        if payload.len() > 8192 {
            return Err(PasswordVaultError::InvalidDocument);
        }
        let wire = Wire {
            version: PASSWORD_VAULT_VERSION,
            vault_id: self.id.as_str().to_owned(),
            kdf: self.kdf(),
            wrapping: Wrapping {
                algorithm: WRAPPING_ALGORITHM.to_owned(),
                nonce: STANDARD.encode(self.nonce),
                encrypted_key: STANDARD.encode(self.encrypted_key),
                tag: STANDARD.encode(self.tag),
            },
            payload,
        };
        let bytes = serde_json::to_vec(&wire).map_err(|_| PasswordVaultError::InvalidDocument)?;
        if bytes.len() > MAX_PASSWORD_VAULT_BYTES {
            return Err(PasswordVaultError::InvalidDocument);
        }
        Ok(bytes)
    }

    /// Opaque host-selected wallet identity authenticated by the wrapper.
    #[must_use]
    pub fn id(&self) -> &VaultKeyId {
        &self.id
    }
    /// Embedded encrypted payload. Its display facts are authenticated only after open.
    #[must_use]
    pub const fn payload(&self) -> &VaultDocument {
        &self.payload
    }
    /// Exact encrypted-document revision used for optimistic write conflict checks.
    ///
    /// # Errors
    /// Returns a closed serialisation error.
    pub fn revision(&self) -> Result<[u8; 32], PasswordVaultError> {
        Ok(Sha256::digest(self.to_json()?).into())
    }

    pub(super) fn aad(&self) -> Result<Vec<u8>, PasswordVaultError> {
        let metadata = Metadata {
            version: PASSWORD_VAULT_VERSION,
            vault_id: self.id.as_str(),
            kdf: self.kdf(),
            wrapping_algorithm: WRAPPING_ALGORITHM,
            nonce: STANDARD.encode(self.nonce),
            payload_sha256: Sha256::digest(
                self.payload
                    .to_json()
                    .map_err(|_| PasswordVaultError::InvalidDocument)?,
            )
            .into(),
        };
        let mut aad = b"minutus/password-vault/dek-wrap/v2\0".to_vec();
        aad.extend(serde_json::to_vec(&metadata).map_err(|_| PasswordVaultError::InvalidDocument)?);
        Ok(aad)
    }
    fn kdf(&self) -> Kdf {
        Kdf {
            algorithm: "argon2id".to_owned(),
            version: 19,
            memory_kib: kdf::MEMORY_KIB,
            iterations: kdf::ITERATIONS,
            lanes: kdf::LANES,
            salt: STANDARD.encode(self.salt),
        }
    }
}

fn decode<const N: usize>(value: &str) -> Result<[u8; N], PasswordVaultError> {
    if value.len() > N * 2 {
        return Err(PasswordVaultError::InvalidDocument);
    }
    let bytes = STANDARD
        .decode(value)
        .map_err(|_| PasswordVaultError::InvalidDocument)?;
    if STANDARD.encode(&bytes) != value {
        return Err(PasswordVaultError::InvalidDocument);
    }
    bytes
        .try_into()
        .map_err(|_| PasswordVaultError::InvalidDocument)
}
