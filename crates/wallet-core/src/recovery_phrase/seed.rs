use zeroize::Zeroizing;

/// Size of a seed derived from a BIP-39 recovery phrase.
const RECOVERY_SEED_BYTES: usize = 64;

/// Zeroizing BIP-39 seed for account-key derivation.
///
/// This type intentionally implements neither `Clone`, `Debug`, formatting nor
/// serialization. Consumers can borrow the bytes only for the duration of an
/// owning cryptographic operation.
pub struct RecoverySeed(Zeroizing<[u8; RECOVERY_SEED_BYTES]>);

impl RecoverySeed {
    pub(crate) fn new(bytes: [u8; RECOVERY_SEED_BYTES]) -> Self {
        Self(Zeroizing::new(bytes))
    }

    /// Borrows the seed for an owning account-key derivation operation.
    #[must_use]
    pub fn expose_secret(&self) -> &[u8; RECOVERY_SEED_BYTES] {
        &self.0
    }
}
