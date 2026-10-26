use std::error::Error;

use serde_json::{Value, json};

use super::*;

const ADDRESS: &str = "0x0123456789abcdef0123456789abcdef01234567";

fn account() -> Result<AccountMetadata, VaultFormatError> {
    AccountMetadata::new(
        "primary",
        ADDRESS,
        84_532,
        AccountKeyOrigin::recovery_phrase("m/44'/60'/0'/0/0")?,
    )
}

fn document() -> Result<VaultDocument, VaultFormatError> {
    VaultDocument::new(
        [7; NONCE_BYTES],
        [11; KDF_SALT_BYTES],
        vec![1, 2, 3, 4],
        [13; AUTHENTICATION_TAG_BYTES],
        vec![account()?],
    )
}

fn wire_value() -> Result<Value, Box<dyn Error>> {
    Ok(serde_json::from_slice(&document()?.to_json()?)?)
}

#[test]
fn version_one_round_trips_every_required_field() -> Result<(), Box<dyn Error>> {
    let original = document()?;
    let json = original.to_json()?;
    let value: Value = serde_json::from_slice(&json)?;

    assert_eq!(value["version"], CURRENT_VAULT_VERSION);
    assert_eq!(value["encryption"]["algorithm"], ENCRYPTION_ALGORITHM);
    assert_eq!(value["kdf"]["algorithm"], KDF_ALGORITHM);
    assert_eq!(value["kdf"]["info"], KDF_INFO);
    assert!(value.get("encryptedPayload").is_some());
    assert!(value.get("authenticationTag").is_some());
    assert_eq!(value["accounts"][0]["address"], ADDRESS);
    assert_eq!(VaultDocument::from_json(&json), Ok(original));
    Ok(())
}

#[test]
fn authenticated_metadata_binds_header_and_accounts_but_not_ciphertext_or_tag()
-> Result<(), Box<dyn Error>> {
    let original = document()?;
    let different_ciphertext = VaultDocument::new(
        *original.nonce(),
        *original.kdf_salt(),
        vec![9, 9],
        [99; AUTHENTICATION_TAG_BYTES],
        original.accounts().to_vec(),
    )?;
    assert_eq!(
        original.authenticated_metadata(),
        different_ciphertext.authenticated_metadata()
    );

    let changed_account = AccountMetadata::new(
        "secondary",
        ADDRESS,
        84_532,
        AccountKeyOrigin::ImportedPrivateKey,
    )?;
    let changed_metadata = VaultDocument::new(
        *original.nonce(),
        *original.kdf_salt(),
        original.encrypted_payload().to_vec(),
        *original.authentication_tag(),
        vec![changed_account],
    )?;
    assert_ne!(
        original.authenticated_metadata(),
        changed_metadata.authenticated_metadata()
    );
    Ok(())
}

#[test]
fn unsupported_versions_and_algorithms_fail_closed() -> Result<(), Box<dyn Error>> {
    let mut future = wire_value()?;
    future["version"] = json!(CURRENT_VAULT_VERSION + 1);
    assert_eq!(
        VaultDocument::from_json(&serde_json::to_vec(&future)?),
        Err(VaultFormatError::UnsupportedVersion {
            found: CURRENT_VAULT_VERSION + 1,
            current: CURRENT_VAULT_VERSION,
        })
    );

    let mut algorithm = wire_value()?;
    algorithm["encryption"]["algorithm"] = json!("aes-256-gcm");
    assert_eq!(
        VaultDocument::from_json(&serde_json::to_vec(&algorithm)?),
        Err(VaultFormatError::UnsupportedParameter {
            field: "encryption.algorithm",
        })
    );
    Ok(())
}

#[test]
fn malformed_binary_fields_are_rejected_before_use() -> Result<(), Box<dyn Error>> {
    let mut nonce = wire_value()?;
    nonce["encryption"]["nonce"] = json!("AQ==");
    assert_eq!(
        VaultDocument::from_json(&serde_json::to_vec(&nonce)?),
        Err(VaultFormatError::InvalidLength {
            field: "encryption.nonce",
            actual: 1,
            expected: NONCE_BYTES,
        })
    );

    let mut tag = wire_value()?;
    tag["authenticationTag"] = json!("not base64");
    assert_eq!(
        VaultDocument::from_json(&serde_json::to_vec(&tag)?),
        Err(VaultFormatError::InvalidEncoding {
            field: "authenticationTag",
        })
    );
    Ok(())
}

#[test]
fn payload_and_account_collections_are_bounded() -> Result<(), Box<dyn Error>> {
    assert_eq!(
        VaultDocument::new(
            [0; NONCE_BYTES],
            [0; KDF_SALT_BYTES],
            Vec::new(),
            [0; AUTHENTICATION_TAG_BYTES],
            vec![account()?],
        ),
        Err(VaultFormatError::InvalidPayloadLength {
            actual: 0,
            maximum: MAX_ENCRYPTED_PAYLOAD_BYTES,
        })
    );
    assert_eq!(
        VaultDocument::new(
            [0; NONCE_BYTES],
            [0; KDF_SALT_BYTES],
            vec![1],
            [0; AUTHENTICATION_TAG_BYTES],
            vec![account()?; MAX_ACCOUNTS + 1],
        ),
        Err(VaultFormatError::InvalidAccountMetadata { field: "accounts" })
    );
    Ok(())
}

#[test]
fn account_metadata_is_validated_on_decode() -> Result<(), Box<dyn Error>> {
    let mut invalid_address = wire_value()?;
    invalid_address["accounts"][0]["address"] = json!("0x0123456789ABCDEF0123456789abcdef01234567");
    assert_eq!(
        VaultDocument::from_json(&serde_json::to_vec(&invalid_address)?),
        Err(VaultFormatError::InvalidAccountMetadata {
            field: "accounts[].address",
        })
    );

    let mut duplicate = wire_value()?;
    let first = duplicate["accounts"][0].clone();
    duplicate["accounts"] = json!([first.clone(), first]);
    assert_eq!(
        VaultDocument::from_json(&serde_json::to_vec(&duplicate)?),
        Err(VaultFormatError::DuplicateAccountId)
    );
    Ok(())
}

#[test]
fn unknown_fields_are_rejected() -> Result<(), Box<dyn Error>> {
    let mut value = wire_value()?;
    value["plaintextMnemonic"] = json!("must not be accepted");
    assert!(matches!(
        VaultDocument::from_json(&serde_json::to_vec(&value)?),
        Err(VaultFormatError::MalformedDocument(_))
    ));
    Ok(())
}
