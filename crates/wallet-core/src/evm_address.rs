use std::fmt::Write as _;

use sha3::{Digest as _, Keccak256};

/// Canonical lowercase public EVM account address.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EvmAddress(String);

impl EvmAddress {
    /// Returns the `0x`-prefixed lowercase public address.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub(crate) fn from_uncompressed_public_key(public_key: &[u8]) -> Option<EvmAddress> {
    if public_key.len() != 65 || public_key.first() != Some(&4) {
        return None;
    }
    let digest = Keccak256::digest(public_key.get(1..)?);
    let address_bytes = digest.get(12..)?;

    let mut address = String::with_capacity(42);
    address.push_str("0x");
    for byte in address_bytes {
        write!(&mut address, "{byte:02x}").ok()?;
    }
    Some(EvmAddress(address))
}
