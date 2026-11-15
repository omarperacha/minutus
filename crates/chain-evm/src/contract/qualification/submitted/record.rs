use crate::EvmNetwork;

/// Non-deserialisable certificate of exact canonical transaction/receipt
/// inclusion at observation time. A successful receipt is not task completion,
/// finality, source qualification or a promise against a subsequent reorg.
/// Only the chain owner's live acquisition and inclusion verifier can mint it.
pub struct VerifiedSubmittedTransaction {
    pub(super) network: EvmNetwork,
    pub(super) transaction_hash: String,
    pub(super) account: String,
    pub(super) nonce: String,
    pub(super) signing_digest: String,
    pub(super) succeeded: bool,
    pub(super) block_number: u64,
    pub(super) block_hash: String,
    pub(super) inclusion_commitment: String,
}

impl VerifiedSubmittedTransaction {
    /// Network independently checked against the configured RPC and signed body.
    #[must_use]
    pub const fn network(&self) -> EvmNetwork {
        self.network
    }
    /// Exact included signed transaction hash, canonical lowercase 0x hex.
    #[must_use]
    pub fn transaction_hash(&self) -> &str {
        &self.transaction_hash
    }
    /// Recovered signer, canonical lowercase 0x address.
    #[must_use]
    pub fn account(&self) -> &str {
        &self.account
    }
    /// Exact body nonce, canonical unsigned decimal.
    #[must_use]
    pub fn nonce(&self) -> &str {
        &self.nonce
    }
    /// Exact unsigned type-2 signing digest, canonical lowercase 0x hex.
    #[must_use]
    pub fn signing_digest(&self) -> &str {
        &self.signing_digest
    }
    /// Exact receipt status. False means included and reverted, not pending.
    #[must_use]
    pub const fn succeeded(&self) -> bool {
        self.succeeded
    }
    /// Included block height, canonical at the final observation check.
    #[must_use]
    pub const fn block_number(&self) -> u64 {
        self.block_number
    }
    /// Complete included header hash.
    #[must_use]
    pub fn block_hash(&self) -> &str {
        &self.block_hash
    }
    /// Domain-separated SHA-256 commitment to exact bindings and both proofs.
    #[must_use]
    pub fn inclusion_commitment(&self) -> &str {
        &self.inclusion_commitment
    }
}
