use crate::{StorageError, StoredEvmTransaction};
use rusqlite::{Connection, params};

/// Opaque exact-owner fence, held by trusted host code through signing/dispatch.
/// Not cloneable, serialisable, model-visible, approval or signing authority.
#[derive(Debug)]
pub struct EvmNonceClaim {
    pub(in crate::store) id: i64,
    pub(in crate::store) network: String,
    pub(in crate::store) sender: String,
    pub(in crate::store) nonce: String,
    pub(in crate::store) candidate_digest: String,
    pub(in crate::store) owner_token: String,
}
impl EvmNonceClaim {
    pub(in crate::store::evm_nonces) fn validated(
        network: &str,
        sender: &str,
        nonce: &str,
        digest: &str,
        owner: &str,
    ) -> Result<Self, StorageError> {
        if !matches!(
            network,
            "eip155:1" | "eip155:11155111" | "eip155:8453" | "eip155:84532"
        ) || sender.len() != 42
            || !sender.starts_with("0x")
            || !sender[2..].bytes().all(|byte| byte.is_ascii_hexdigit())
            || nonce.is_empty()
            || nonce.len() > 78
            || (nonce.len() > 1 && nonce.starts_with('0'))
            || !nonce.bytes().all(|byte| byte.is_ascii_digit())
            || digest.len() != 66
            || !digest.starts_with("0x")
            || !digest[2..]
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || owner.is_empty()
            || owner.len() > 160
            || !owner
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(StorageError::InvalidRecord(
                "invalid nonce ownership binding",
            ));
        }
        Ok(Self {
            id: 0,
            network: network.into(),
            sender: sender.to_ascii_lowercase(),
            nonce: nonce.into(),
            candidate_digest: digest.into(),
            owner_token: owner.into(),
        })
    }
    pub(in crate::store) fn matches(&self, record: &StoredEvmTransaction) -> bool {
        self.network == record.network
            && self.sender.eq_ignore_ascii_case(&record.sender)
            && self.nonce == record.nonce
            && self.candidate_digest == record.candidate_digest
    }
}
pub(in crate::store) fn is_reserved(
    connection: &Connection,
    claim: &EvmNonceClaim,
) -> Result<bool, StorageError> {
    Ok(connection.query_row("SELECT EXISTS(SELECT 1 FROM evm_nonce_claims WHERE id=?1 AND network=?2 AND sender=?3 AND nonce=?4 AND candidate_digest=?5 AND owner_token=?6 AND phase='reserved')",params![claim.id,claim.network,claim.sender,claim.nonce,claim.candidate_digest,claim.owner_token],|row|row.get(0))?)
}
pub(in crate::store) fn retire_unsent(
    connection: &Connection,
    claim: &EvmNonceClaim,
) -> Result<(), StorageError> {
    connection.execute("DELETE FROM evm_transactions WHERE transaction_hash IN (SELECT h.transaction_hash FROM evm_nonce_hashes h JOIN evm_transfer_dispatches d USING(transaction_hash) WHERE h.claim_id=?1 AND d.phase='reserved' AND d.reservation_id=?2)",params![claim.id,claim.owner_token])?;
    connection.execute("UPDATE evm_nonce_claims SET phase='retired' WHERE id=?1 AND owner_token=?2 AND phase='reserved'",params![claim.id,claim.owner_token])?;
    Ok(())
}
