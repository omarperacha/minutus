use std::collections::HashSet;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use serde::{Deserialize, Serialize};

use super::{
    AUTHENTICATION_TAG_BYTES, CURRENT_VAULT_VERSION, ENCRYPTION_ALGORITHM, KDF_ALGORITHM, KDF_INFO,
    KDF_SALT_BYTES, MAX_ACCOUNT_ID_BYTES, MAX_ACCOUNTS, MAX_DERIVATION_PATH_BYTES,
    MAX_DOCUMENT_BYTES, MAX_ENCRYPTED_PAYLOAD_BYTES, NONCE_BYTES, VaultFormatError,
};

/// Public recovery origin needed to explain an account's backup boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum AccountKeyOrigin {
    /// Account derived from the app recovery phrase at this BIP-32 path.
    RecoveryPhrase {
        /// BIP-32 derivation path used to recover this account.
        #[serde(rename = "derivationPath")]
        derivation_path: String,
    },
    /// Account recoverable only from its separately imported private key.
    ImportedPrivateKey,
}

impl AccountKeyOrigin {
    /// Creates recovery-phrase origin metadata after validating its bounded path.
    ///
    /// # Errors
    ///
    /// Returns [`VaultFormatError::InvalidAccountMetadata`] when the path is not
    /// a bounded absolute numeric BIP-32 path.
    pub fn recovery_phrase(derivation_path: impl Into<String>) -> Result<Self, VaultFormatError> {
        let derivation_path = derivation_path.into();
        validate_derivation_path(&derivation_path)?;
        Ok(Self::RecoveryPhrase { derivation_path })
    }

    /// Returns the derivation path for a recovery-phrase account.
    #[must_use]
    pub fn derivation_path(&self) -> Option<&str> {
        match self {
            Self::RecoveryPhrase { derivation_path } => Some(derivation_path),
            Self::ImportedPrivateKey => None,
        }
    }
}

/// Non-secret account identity authenticated with the encrypted payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountMetadata {
    account_id: String,
    address: String,
    chain_id: u64,
    key_origin: AccountKeyOrigin,
}

impl AccountMetadata {
    /// Creates validated public EVM account metadata.
    ///
    /// # Errors
    ///
    /// Returns [`VaultFormatError::InvalidAccountMetadata`] when any field is
    /// empty, out of bounds or non-canonical.
    pub fn new(
        account_id: impl Into<String>,
        address: impl Into<String>,
        chain_id: u64,
        key_origin: AccountKeyOrigin,
    ) -> Result<Self, VaultFormatError> {
        let metadata = Self {
            account_id: account_id.into(),
            address: address.into(),
            chain_id,
            key_origin,
        };
        metadata.validate()?;
        Ok(metadata)
    }

    /// Stable opaque account identifier.
    #[must_use]
    pub fn account_id(&self) -> &str {
        &self.account_id
    }

    /// Lowercase `0x`-prefixed EVM address.
    #[must_use]
    pub fn address(&self) -> &str {
        &self.address
    }

    /// EIP-155 chain identifier used to present the account.
    #[must_use]
    pub const fn chain_id(&self) -> u64 {
        self.chain_id
    }

    /// Recovery origin for this account.
    #[must_use]
    pub const fn key_origin(&self) -> &AccountKeyOrigin {
        &self.key_origin
    }

    fn validate(&self) -> Result<(), VaultFormatError> {
        if self.account_id.is_empty()
            || self.account_id.len() > MAX_ACCOUNT_ID_BYTES
            || !self
                .account_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(VaultFormatError::InvalidAccountMetadata {
                field: "accounts[].accountId",
            });
        }
        if !is_canonical_evm_address(&self.address) {
            return Err(VaultFormatError::InvalidAccountMetadata {
                field: "accounts[].address",
            });
        }
        if self.chain_id == 0 {
            return Err(VaultFormatError::InvalidAccountMetadata {
                field: "accounts[].chainId",
            });
        }
        if let AccountKeyOrigin::RecoveryPhrase { derivation_path } = &self.key_origin {
            validate_derivation_path(derivation_path)?;
        }
        Ok(())
    }
}

/// Validated version-1 encrypted vault document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultDocument {
    nonce: [u8; NONCE_BYTES],
    kdf_salt: [u8; KDF_SALT_BYTES],
    encrypted_payload: Vec<u8>,
    authentication_tag: [u8; AUTHENTICATION_TAG_BYTES],
    accounts: Vec<AccountMetadata>,
}

impl VaultDocument {
    /// Constructs a version-1 document from the output of authenticated encryption.
    ///
    /// # Errors
    ///
    /// Returns a typed format error when the ciphertext or account collection
    /// violates a version-1 bound or metadata invariant.
    pub fn new(
        nonce: [u8; NONCE_BYTES],
        kdf_salt: [u8; KDF_SALT_BYTES],
        encrypted_payload: Vec<u8>,
        authentication_tag: [u8; AUTHENTICATION_TAG_BYTES],
        accounts: Vec<AccountMetadata>,
    ) -> Result<Self, VaultFormatError> {
        validate_payload_length(encrypted_payload.len())?;
        validate_accounts(&accounts)?;
        Ok(Self {
            nonce,
            kdf_salt,
            encrypted_payload,
            authentication_tag,
            accounts,
        })
    }

    /// Decodes and validates an untrusted serialized vault document.
    ///
    /// # Errors
    ///
    /// Returns a typed format error for oversized or malformed JSON, unsupported
    /// versions or parameters, invalid binary fields, or invalid metadata.
    pub fn from_json(bytes: &[u8]) -> Result<Self, VaultFormatError> {
        if bytes.len() > MAX_DOCUMENT_BYTES {
            return Err(VaultFormatError::DocumentTooLarge {
                actual: bytes.len(),
                maximum: MAX_DOCUMENT_BYTES,
            });
        }
        let wire: VaultWire = serde_json::from_slice(bytes)
            .map_err(|error| VaultFormatError::MalformedDocument(error.to_string()))?;
        wire.try_into()
    }

    /// Serializes the validated document using the version-1 field layout.
    ///
    /// # Errors
    ///
    /// Returns [`VaultFormatError::MalformedDocument`] if serialization fails.
    pub fn to_json(&self) -> Result<Vec<u8>, VaultFormatError> {
        serde_json::to_vec(&VaultWire::from(self))
            .map_err(|error| VaultFormatError::MalformedDocument(error.to_string()))
    }

    /// Returns the canonical bytes supplied as AEAD additional authenticated data.
    ///
    /// The encrypted payload and tag are deliberately absent. The AEAD operation
    /// authenticates the ciphertext itself; these bytes bind its interpretation,
    /// nonce, key derivation and public account presentation.
    ///
    /// # Errors
    ///
    /// Returns [`VaultFormatError::MalformedDocument`] if serialization fails.
    pub fn authenticated_metadata(&self) -> Result<Vec<u8>, VaultFormatError> {
        serde_json::to_vec(&AuthenticatedMetadataWire::from(self))
            .map_err(|error| VaultFormatError::MalformedDocument(error.to_string()))
    }

    /// Unique nonce used for this encrypted payload.
    #[must_use]
    pub const fn nonce(&self) -> &[u8; NONCE_BYTES] {
        &self.nonce
    }

    /// Salt used to derive the per-vault encryption key from the random DEK.
    #[must_use]
    pub const fn kdf_salt(&self) -> &[u8; KDF_SALT_BYTES] {
        &self.kdf_salt
    }

    /// Authenticated ciphertext containing the secret wallet record.
    #[must_use]
    pub fn encrypted_payload(&self) -> &[u8] {
        &self.encrypted_payload
    }

    /// Detached Poly1305 authentication tag.
    #[must_use]
    pub const fn authentication_tag(&self) -> &[u8; AUTHENTICATION_TAG_BYTES] {
        &self.authentication_tag
    }

    /// Authenticated public account records available while locked.
    #[must_use]
    pub fn accounts(&self) -> &[AccountMetadata] {
        &self.accounts
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VaultWire {
    version: u32,
    encryption: EncryptionWire,
    kdf: KdfWire,
    encrypted_payload: String,
    authentication_tag: String,
    accounts: Vec<AccountMetadataWire>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EncryptionWire {
    algorithm: String,
    nonce: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct KdfWire {
    algorithm: String,
    salt: String,
    info: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthenticatedMetadataWire<'a> {
    version: u32,
    encryption: EncryptionWire,
    kdf: KdfWire,
    accounts: &'a [AccountMetadata],
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum AccountKeyOriginWire {
    RecoveryPhrase {
        #[serde(rename = "derivationPath")]
        derivation_path: String,
    },
    ImportedPrivateKey,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AccountMetadataWire {
    account_id: String,
    address: String,
    chain_id: u64,
    key_origin: AccountKeyOriginWire,
}

impl TryFrom<VaultWire> for VaultDocument {
    type Error = VaultFormatError;

    fn try_from(wire: VaultWire) -> Result<Self, Self::Error> {
        if wire.version != CURRENT_VAULT_VERSION {
            return Err(VaultFormatError::UnsupportedVersion {
                found: wire.version,
                current: CURRENT_VAULT_VERSION,
            });
        }
        require_parameter(
            wire.encryption.algorithm == ENCRYPTION_ALGORITHM,
            "encryption.algorithm",
        )?;
        require_parameter(wire.kdf.algorithm == KDF_ALGORITHM, "kdf.algorithm")?;
        require_parameter(wire.kdf.info == KDF_INFO, "kdf.info")?;

        let nonce = decode_array::<NONCE_BYTES>(&wire.encryption.nonce, "encryption.nonce")?;
        let kdf_salt = decode_array::<KDF_SALT_BYTES>(&wire.kdf.salt, "kdf.salt")?;
        let encrypted_payload = decode_base64(&wire.encrypted_payload, "encryptedPayload")?;
        let authentication_tag = decode_array::<AUTHENTICATION_TAG_BYTES>(
            &wire.authentication_tag,
            "authenticationTag",
        )?;
        let accounts = wire
            .accounts
            .into_iter()
            .map(AccountMetadata::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        Self::new(
            nonce,
            kdf_salt,
            encrypted_payload,
            authentication_tag,
            accounts,
        )
    }
}

impl From<&VaultDocument> for VaultWire {
    fn from(document: &VaultDocument) -> Self {
        Self {
            version: CURRENT_VAULT_VERSION,
            encryption: EncryptionWire {
                algorithm: ENCRYPTION_ALGORITHM.to_owned(),
                nonce: BASE64_STANDARD.encode(document.nonce),
            },
            kdf: KdfWire {
                algorithm: KDF_ALGORITHM.to_owned(),
                salt: BASE64_STANDARD.encode(document.kdf_salt),
                info: KDF_INFO.to_owned(),
            },
            encrypted_payload: BASE64_STANDARD.encode(&document.encrypted_payload),
            authentication_tag: BASE64_STANDARD.encode(document.authentication_tag),
            accounts: document
                .accounts
                .iter()
                .map(AccountMetadataWire::from)
                .collect(),
        }
    }
}

impl TryFrom<AccountMetadataWire> for AccountMetadata {
    type Error = VaultFormatError;

    fn try_from(wire: AccountMetadataWire) -> Result<Self, Self::Error> {
        let key_origin = match wire.key_origin {
            AccountKeyOriginWire::RecoveryPhrase { derivation_path } => {
                AccountKeyOrigin::recovery_phrase(derivation_path)?
            }
            AccountKeyOriginWire::ImportedPrivateKey => AccountKeyOrigin::ImportedPrivateKey,
        };
        Self::new(wire.account_id, wire.address, wire.chain_id, key_origin)
    }
}

impl From<&AccountMetadata> for AccountMetadataWire {
    fn from(metadata: &AccountMetadata) -> Self {
        Self {
            account_id: metadata.account_id.clone(),
            address: metadata.address.clone(),
            chain_id: metadata.chain_id,
            key_origin: AccountKeyOriginWire::from(&metadata.key_origin),
        }
    }
}

impl From<&AccountKeyOrigin> for AccountKeyOriginWire {
    fn from(origin: &AccountKeyOrigin) -> Self {
        match origin {
            AccountKeyOrigin::RecoveryPhrase { derivation_path } => Self::RecoveryPhrase {
                derivation_path: derivation_path.clone(),
            },
            AccountKeyOrigin::ImportedPrivateKey => Self::ImportedPrivateKey,
        }
    }
}

impl<'a> From<&'a VaultDocument> for AuthenticatedMetadataWire<'a> {
    fn from(document: &'a VaultDocument) -> Self {
        Self {
            version: CURRENT_VAULT_VERSION,
            encryption: EncryptionWire {
                algorithm: ENCRYPTION_ALGORITHM.to_owned(),
                nonce: BASE64_STANDARD.encode(document.nonce),
            },
            kdf: KdfWire {
                algorithm: KDF_ALGORITHM.to_owned(),
                salt: BASE64_STANDARD.encode(document.kdf_salt),
                info: KDF_INFO.to_owned(),
            },
            accounts: &document.accounts,
        }
    }
}

fn validate_accounts(accounts: &[AccountMetadata]) -> Result<(), VaultFormatError> {
    if accounts.is_empty() || accounts.len() > MAX_ACCOUNTS {
        return Err(VaultFormatError::InvalidAccountMetadata { field: "accounts" });
    }
    let mut account_ids = HashSet::with_capacity(accounts.len());
    for account in accounts {
        account.validate()?;
        if !account_ids.insert(account.account_id()) {
            return Err(VaultFormatError::DuplicateAccountId);
        }
    }
    Ok(())
}

fn validate_payload_length(length: usize) -> Result<(), VaultFormatError> {
    if length == 0 || length > MAX_ENCRYPTED_PAYLOAD_BYTES {
        return Err(VaultFormatError::InvalidPayloadLength {
            actual: length,
            maximum: MAX_ENCRYPTED_PAYLOAD_BYTES,
        });
    }
    Ok(())
}

fn validate_derivation_path(path: &str) -> Result<(), VaultFormatError> {
    let mut components = path.split('/');
    let root_is_valid = components.next() == Some("m");
    let components_are_valid = components.clone().count() > 0
        && components.all(|component| {
            let digits = component.strip_suffix('\'').unwrap_or(component);
            !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
        });
    if !root_is_valid
        || path.len() > MAX_DERIVATION_PATH_BYTES
        || !path.is_ascii()
        || !components_are_valid
    {
        return Err(VaultFormatError::InvalidAccountMetadata {
            field: "accounts[].keyOrigin.derivationPath",
        });
    }
    Ok(())
}

fn is_canonical_evm_address(address: &str) -> bool {
    address.len() == 42
        && address.starts_with("0x")
        && address[2..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn require_parameter(condition: bool, field: &'static str) -> Result<(), VaultFormatError> {
    if condition {
        Ok(())
    } else {
        Err(VaultFormatError::UnsupportedParameter { field })
    }
}

fn decode_base64(value: &str, field: &'static str) -> Result<Vec<u8>, VaultFormatError> {
    let decoded = BASE64_STANDARD
        .decode(value)
        .map_err(|_| VaultFormatError::InvalidEncoding { field })?;
    if BASE64_STANDARD.encode(&decoded) != value {
        return Err(VaultFormatError::InvalidEncoding { field });
    }
    Ok(decoded)
}

fn decode_array<const LENGTH: usize>(
    value: &str,
    field: &'static str,
) -> Result<[u8; LENGTH], VaultFormatError> {
    let decoded = decode_base64(value, field)?;
    let actual = decoded.len();
    decoded
        .try_into()
        .map_err(|_| VaultFormatError::InvalidLength {
            field,
            actual,
            expected: LENGTH,
        })
}
