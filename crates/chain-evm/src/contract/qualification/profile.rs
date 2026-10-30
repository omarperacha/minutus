//! This compiled-in source/artifact root is reviewed code policy, not supplied JSON.
use super::QualificationError;
use alloy_primitives::keccak256;
use sha2::{Digest, Sha256};

pub(super) const ID: &str = "oz570-solc0838-cancun-erc20-v1";
pub(super) const REVISION: &str =
    "76df8832488cb0beac36a11b032a1240a39f05202b4c604d580e0485645feef7";
const RUNTIME_HASH: &str = "0x34ad9dbf5175a1e9da19c8edb9d390a0026ac815cdf159a903a04f53e39fd841";
const MANIFEST: &[u8] = include_bytes!("../../../profiles/erc20-oz-5.7.0/manifest.json");
const INPUT: &[u8] = include_bytes!("../../../profiles/erc20-oz-5.7.0/input.json");
const ABI: &[u8] = include_bytes!("../../../profiles/erc20-oz-5.7.0/abi.json");
const LAYOUT: &[u8] = include_bytes!("../../../profiles/erc20-oz-5.7.0/storage-layout.json");
pub(super) fn verify(code: &[u8]) -> Result<(), QualificationError> {
    if code.len() != 1764 || format!("{:#x}", keccak256(code)) != RUNTIME_HASH {
        return Err(QualificationError::UnsupportedProfile);
    }
    for (data, expected) in [
        (
            MANIFEST,
            "7f90b52883050e63b66d7215d0571f0d18cc3b7f2ab5d299074200af7568044b",
        ),
        (
            INPUT,
            "f301d4a85ef7faa3cee8fd6050df28e11ae8a3355ed8c285bb1b72f461978e04",
        ),
        (
            ABI,
            "e15a112bebf7604c58292463ae45181ac86e0fb3c83204fac748a26f4ab7952d",
        ),
        (
            LAYOUT,
            "4225a39becd4ee2e5a9bea6a4875bb10f740bda402010acd51914b09551fcd0f",
        ),
    ] {
        if format!("{:x}", Sha256::digest(data)) != expected {
            return Err(QualificationError::InvalidArtifact);
        }
    }
    let manifest: serde_json::Value =
        serde_json::from_slice(MANIFEST).map_err(|_| QualificationError::InvalidArtifact)?;
    let mut identity_fields = std::collections::BTreeMap::new();
    for name in [
        "schemaVersion",
        "profileId",
        "sourceRevision",
        "sourceBundleSha256",
        "compilerSha256",
        "inputSha256",
        "runtimeKeccak256",
        "abiSha256",
        "storageLayoutSha256",
    ] {
        identity_fields.insert(
            name,
            manifest
                .get(name)
                .ok_or(QualificationError::InvalidArtifact)?,
        );
    }
    let encoded_identity =
        serde_json::to_vec(&identity_fields).map_err(|_| QualificationError::InvalidArtifact)?;
    if format!("{:x}", Sha256::digest(encoded_identity)) != REVISION {
        return Err(QualificationError::InvalidArtifact);
    }
    Ok(())
}
pub(super) const fn runtime_hash() -> &'static str {
    RUNTIME_HASH
}
