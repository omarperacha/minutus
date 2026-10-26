use bip32::secp256k1::{SecretKey, elliptic_curve::sec1::ToSec1Point as _};
use zeroize::Zeroizing;

use crate::{EvmAddress, evm_address::from_uncompressed_public_key};

use super::EvmPrivateKeyImportError;

const EVM_PRIVATE_KEY_BYTES: usize = 32;

/// Validated, zeroizing secp256k1 private key for encrypted vault import.
///
/// This type intentionally implements neither cloning, debugging, formatting
/// nor serialization. Bytes are exposed only to the owning vault writer.
pub struct ImportedEvmPrivateKey(Zeroizing<[u8; EVM_PRIVATE_KEY_BYTES]>);

impl ImportedEvmPrivateKey {
    pub(crate) fn from_bytes(
        bytes: [u8; EVM_PRIVATE_KEY_BYTES],
    ) -> Result<Self, EvmPrivateKeyImportError> {
        SecretKey::from_slice(&bytes).map_err(|_| EvmPrivateKeyImportError::ScalarOutOfRange)?;
        Ok(Self(Zeroizing::new(bytes)))
    }

    pub(super) fn parse(encoded: &str) -> Result<Self, EvmPrivateKeyImportError> {
        let encoded = encoded
            .strip_prefix("0x")
            .or_else(|| encoded.strip_prefix("0X"))
            .unwrap_or(encoded);
        if encoded.len() != EVM_PRIVATE_KEY_BYTES * 2 {
            return Err(EvmPrivateKeyImportError::InvalidEncoding);
        }

        let mut bytes = Zeroizing::new([0_u8; EVM_PRIVATE_KEY_BYTES]);
        for (index, pair) in encoded.as_bytes().chunks_exact(2).enumerate() {
            let high = decode_nibble(pair[0]).ok_or(EvmPrivateKeyImportError::InvalidEncoding)?;
            let low = decode_nibble(pair[1]).ok_or(EvmPrivateKeyImportError::InvalidEncoding)?;
            bytes[index] = (high << 4) | low;
        }
        SecretKey::from_slice(bytes.as_ref())
            .map_err(|_| EvmPrivateKeyImportError::ScalarOutOfRange)?;
        Ok(Self(bytes))
    }

    pub(super) fn address(&self) -> Result<EvmAddress, EvmPrivateKeyImportError> {
        let secret_key = SecretKey::from_slice(self.0.as_ref())
            .map_err(|_| EvmPrivateKeyImportError::ScalarOutOfRange)?;
        let public_key = secret_key.public_key().to_sec1_point(false);
        from_uncompressed_public_key(public_key.as_bytes())
            .ok_or(EvmPrivateKeyImportError::AddressDerivationFailed)
    }

    /// Borrows the key only for the owning encrypted-vault write operation.
    #[must_use]
    pub(crate) fn expose_secret(&self) -> &[u8; EVM_PRIVATE_KEY_BYTES] {
        &self.0
    }
}

const fn decode_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
