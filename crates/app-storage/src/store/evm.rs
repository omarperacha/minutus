use super::{
    ApplicationStore, OptionalExtension, PublicWalletMetadata, StorageError,
    StoredApprovalFinalization, StoredApprovalFinalizationOutcome, StoredEvmTransaction,
    StoredEvmTransactionState, WalletId, params,
};

impl ApplicationStore {
    /// Inserts or updates public wallet metadata only.
    ///
    /// # Errors
    ///
    /// Returns an error when the database write fails.
    pub fn save_public_wallet(&self, wallet: &PublicWalletMetadata) -> Result<(), StorageError> {
        self.connection.execute(
            "INSERT INTO public_wallets (id, address, network, availability, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
                address = excluded.address,
                network = excluded.network,
                availability = excluded.availability,
                updated_at = excluded.updated_at",
            params![
                wallet.id.0,
                wallet.address,
                wallet.network,
                if wallet.locked { "locked" } else { "ready" },
                wallet.updated_at
            ],
        )?;
        Ok(())
    }

    /// Removes one exact public metadata record after explicit account removal.
    /// This never accesses encrypted wallet material.
    /// # Errors
    /// Returns a sanitised database failure.
    pub fn remove_public_wallet(&self, wallet_id: &WalletId) -> Result<(), StorageError> {
        self.connection
            .execute("DELETE FROM public_wallets WHERE id = ?1", [&wallet_id.0])?;
        Ok(())
    }

    /// Loads public wallet metadata without touching the separate wallet vault.
    ///
    /// # Errors
    ///
    /// Returns an error when the record is malformed or the database read fails.
    pub fn public_wallet(
        &self,
        wallet_id: &WalletId,
    ) -> Result<Option<PublicWalletMetadata>, StorageError> {
        self.connection
            .query_row(
                "SELECT address, network, availability, updated_at
                 FROM public_wallets WHERE id = ?1",
                [&wallet_id.0],
                |row| {
                    let availability: String = row.get(2)?;
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        availability,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?
            .map(|(address, network, availability, updated_at)| {
                let locked = match availability.as_str() {
                    "locked" => true,
                    "ready" => false,
                    _ => return Err(StorageError::CorruptDatabase),
                };
                Ok(PublicWalletMetadata {
                    id: wallet_id.clone(),
                    address,
                    network,
                    locked,
                    updated_at,
                })
            })
            .transpose()
    }

    /// Loads the most recently selected public wallet record.
    ///
    /// # Errors
    ///
    /// Returns a database or strict record-validation failure.
    pub fn latest_public_wallet(&self) -> Result<Option<PublicWalletMetadata>, StorageError> {
        let id = self
            .connection
            .query_row(
                "SELECT id FROM public_wallets ORDER BY updated_at DESC, id DESC LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        id.map(|id| {
            let id = WalletId(id);
            self.public_wallet(&id)?
                .ok_or(StorageError::CorruptDatabase)
        })
        .transpose()
    }

    /// Inserts or updates non-secret metadata for one submitted EVM transaction.
    ///
    /// Signed bytes, signatures, signing payloads and approval capabilities are
    /// structurally absent from this record.
    ///
    /// # Errors
    ///
    /// Returns an error when metadata is malformed or the database write fails.
    pub fn save_evm_transaction(
        &self,
        transaction: &StoredEvmTransaction,
    ) -> Result<(), StorageError> {
        write_evm_transaction(&self.connection, transaction)
    }

    /// Lists submitted EVM transactions in most-recently-updated order.
    ///
    /// # Errors
    ///
    /// Returns an error when persisted metadata is malformed or cannot be read.
    pub fn evm_transactions(&self) -> Result<Vec<StoredEvmTransaction>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT transaction_hash, network, sender, nonce, candidate_digest,
                    state, block_number, block_hash, submitted_at, updated_at
             FROM evm_transactions
             ORDER BY updated_at DESC, transaction_hash",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|row| {
                let transaction = StoredEvmTransaction {
                    transaction_hash: row.0,
                    network: row.1,
                    sender: row.2,
                    nonce: row.3,
                    candidate_digest: row.4,
                    state: parse_evm_transaction_state(&row.5)?,
                    block_number: row.6,
                    block_hash: row.7,
                    submitted_at: row.8,
                    updated_at: row.9,
                };
                validate_evm_transaction(&transaction)?;
                Ok(transaction)
            })
            .collect()
    }

    /// Atomically records the first terminal decision for one exact approval.
    ///
    /// The primary-key insert is the cross-process serialization point. Rows
    /// are append-only through this API, so restart and duplicate delivery
    /// cannot restore approval authority.
    ///
    /// # Errors
    ///
    /// Returns an error when the bounded public record is malformed or `SQLite`
    /// cannot durably establish or read the terminal decision.
    pub fn finalize_transaction_approval(
        &self,
        record: &StoredApprovalFinalization,
    ) -> Result<StoredApprovalFinalizationOutcome, StorageError> {
        validate_approval_finalization(record)?;
        let inserted = self.connection.execute(
            "INSERT INTO transaction_approval_finalizations (
                approval_id, commitment_digest, signing_digest, chain_id,
                account, session_id, expires_at_unix, transaction_type,
                policy_revision, disposition, invalidation_reason,
                finalized_at_unix
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(approval_id) DO NOTHING",
            params![
                record.approval_id,
                record.commitment_digest,
                record.signing_digest,
                record.chain_id,
                record.account,
                record.session_id,
                record.expires_at_unix,
                record.transaction_type,
                record.policy_revision,
                record.disposition,
                record.invalidation_reason,
                record.finalized_at_unix,
            ],
        )?;
        if inserted == 1 {
            return Ok(StoredApprovalFinalizationOutcome::Finalized);
        }

        let existing = self.connection.query_row(
            "SELECT commitment_digest, signing_digest, chain_id, account,
                    session_id, expires_at_unix, transaction_type,
                    policy_revision, disposition
             FROM transaction_approval_finalizations WHERE approval_id = ?1",
            [&record.approval_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                ))
            },
        )?;
        let exact_binding = existing.0 == record.commitment_digest
            && existing.1 == record.signing_digest
            && existing.2 == record.chain_id
            && existing.3 == record.account
            && existing.4 == record.session_id
            && existing.5 == record.expires_at_unix
            && existing.6 == record.transaction_type
            && existing.7 == record.policy_revision;
        if !exact_binding {
            let outcome = StoredApprovalFinalizationOutcome::IdentifierCollision;
            self.append_approval_replay_event(record, outcome)?;
            return Ok(outcome);
        }
        let outcome = match existing.8.as_str() {
            "consumed" => StoredApprovalFinalizationOutcome::AlreadyConsumed,
            "invalidated" => StoredApprovalFinalizationOutcome::Invalidated,
            _ => return Err(StorageError::CorruptDatabase),
        };
        self.append_approval_replay_event(record, outcome)?;
        Ok(outcome)
    }

    fn append_approval_replay_event(
        &self,
        record: &StoredApprovalFinalization,
        outcome: StoredApprovalFinalizationOutcome,
    ) -> Result<(), StorageError> {
        let outcome = match outcome {
            StoredApprovalFinalizationOutcome::AlreadyConsumed => "already_consumed",
            StoredApprovalFinalizationOutcome::Invalidated => "invalidated",
            StoredApprovalFinalizationOutcome::IdentifierCollision => "identifier_collision",
            StoredApprovalFinalizationOutcome::Finalized => {
                return Err(StorageError::InvalidRecord(
                    "successful approval finalization is not a replay event",
                ));
            }
        };
        self.connection.execute(
            "INSERT INTO transaction_approval_replay_events (
                approval_id, attempted_commitment_digest, outcome, occurred_at_unix
             ) VALUES (?1, ?2, ?3, ?4)",
            params![
                record.approval_id,
                record.commitment_digest,
                outcome,
                record.finalized_at_unix,
            ],
        )?;
        Ok(())
    }
}

const fn evm_transaction_state_name(state: StoredEvmTransactionState) -> &'static str {
    match state {
        StoredEvmTransactionState::Pending => "pending",
        StoredEvmTransactionState::Confirmed => "confirmed",
        StoredEvmTransactionState::Failed => "failed",
        StoredEvmTransactionState::DroppedOrReplaced => "dropped_or_replaced",
    }
}

fn parse_evm_transaction_state(value: &str) -> Result<StoredEvmTransactionState, StorageError> {
    match value {
        "pending" => Ok(StoredEvmTransactionState::Pending),
        "confirmed" => Ok(StoredEvmTransactionState::Confirmed),
        "failed" => Ok(StoredEvmTransactionState::Failed),
        "dropped_or_replaced" => Ok(StoredEvmTransactionState::DroppedOrReplaced),
        _ => Err(StorageError::CorruptDatabase),
    }
}

fn validate_evm_transaction(transaction: &StoredEvmTransaction) -> Result<(), StorageError> {
    let valid_network = matches!(
        transaction.network.as_str(),
        "eip155:1" | "eip155:11155111" | "eip155:8453" | "eip155:84532"
    );
    let valid_nonce = !transaction.nonce.is_empty()
        && transaction.nonce.len() <= 78
        && (transaction.nonce == "0" || !transaction.nonce.starts_with('0'))
        && transaction.nonce.bytes().all(|byte| byte.is_ascii_digit());
    let terminal = matches!(
        transaction.state,
        StoredEvmTransactionState::Confirmed | StoredEvmTransactionState::Failed
    );
    let block_pair = match (&transaction.block_number, &transaction.block_hash) {
        (Some(number), Some(hash)) => {
            !number.is_empty()
                && number.len() <= 78
                && number.bytes().all(|byte| byte.is_ascii_digit())
                && valid_lower_hex(hash, 32)
        }
        (None, None) => false,
        _ => {
            return Err(StorageError::InvalidRecord(
                "EVM block metadata is incomplete",
            ));
        }
    };
    if !valid_network
        || !valid_lower_hex(&transaction.transaction_hash, 32)
        || !valid_evm_address(&transaction.sender)
        || !valid_nonce
        || !valid_lower_hex(&transaction.candidate_digest, 32)
        || terminal != block_pair
        || !(1..=64).contains(&transaction.submitted_at.len())
        || !(1..=64).contains(&transaction.updated_at.len())
    {
        return Err(StorageError::InvalidRecord(
            "EVM transaction metadata is invalid",
        ));
    }
    Ok(())
}

fn validate_approval_finalization(record: &StoredApprovalFinalization) -> Result<(), StorageError> {
    let valid_disposition = match (record.disposition.as_str(), &record.invalidation_reason) {
        ("consumed", None) => true,
        ("invalidated", Some(reason)) => matches!(
            reason.as_str(),
            "cancelled" | "expired" | "failed" | "superseded"
        ),
        _ => false,
    };
    if !valid_bare_lower_hex(&record.approval_id, 32)
        || !valid_bare_lower_hex(&record.commitment_digest, 32)
        || !valid_bare_lower_hex(&record.signing_digest, 32)
        || !valid_decimal(&record.chain_id, 20, false)
        || !valid_evm_address(&record.account)
        || record.session_id.is_empty()
        || record.session_id.len() > 128
        || !record
            .session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        || !valid_decimal(&record.expires_at_unix, 20, false)
        || !matches!(record.transaction_type.as_str(), "eip1559:2" | "eip712")
        || !valid_decimal(&record.policy_revision, 10, false)
        || !valid_disposition
        || !valid_decimal(&record.finalized_at_unix, 20, true)
    {
        return Err(StorageError::InvalidRecord(
            "transaction approval finalization metadata is invalid",
        ));
    }
    Ok(())
}

fn valid_bare_lower_hex(value: &str, bytes: usize) -> bool {
    value.len() == bytes * 2
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_decimal(value: &str, max_bytes: usize, allow_zero: bool) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && (value == "0" || !value.starts_with('0'))
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && (allow_zero || value != "0")
}

fn valid_lower_hex(value: &str, bytes: usize) -> bool {
    value.strip_prefix("0x").is_some_and(|hex| {
        hex.len() == bytes * 2
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn valid_evm_address(value: &str) -> bool {
    value
        .strip_prefix("0x")
        .is_some_and(|hex| hex.len() == 40 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

pub(super) fn write_evm_transaction(
    connection: &rusqlite::Connection,
    transaction: &StoredEvmTransaction,
) -> Result<(), StorageError> {
    validate_evm_transaction(transaction)?;
    connection.execute(
        "INSERT INTO evm_transactions (
                transaction_hash, network, sender, nonce, candidate_digest,
                state, block_number, block_hash, submitted_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(transaction_hash) DO UPDATE SET
                network = excluded.network,
                sender = excluded.sender,
                nonce = excluded.nonce,
                candidate_digest = excluded.candidate_digest,
                state = excluded.state,
                block_number = excluded.block_number,
                block_hash = excluded.block_hash,
                submitted_at = excluded.submitted_at,
                updated_at = excluded.updated_at",
        params![
            transaction.transaction_hash,
            transaction.network,
            transaction.sender,
            transaction.nonce,
            transaction.candidate_digest,
            evm_transaction_state_name(transaction.state),
            transaction.block_number,
            transaction.block_hash,
            transaction.submitted_at,
            transaction.updated_at,
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod public_wallet_removal_tests {
    use super::*;
    #[test]
    fn removing_exact_public_metadata_preserves_other_accounts()
    -> Result<(), Box<dyn std::error::Error>> {
        let store = ApplicationStore::open_in_memory()?;
        for id in ["first", "second"] {
            store.save_public_wallet(&PublicWalletMetadata {
                id: WalletId(id.to_owned()),
                address: "0x1234".to_owned(),
                network: "eip155:8453".to_owned(),
                locked: true,
                updated_at: "2026-10-21T00:00:00Z".to_owned(),
            })?;
        }
        store.remove_public_wallet(&WalletId("second".to_owned()))?;
        assert!(
            store
                .public_wallet(&WalletId("second".to_owned()))?
                .is_none()
        );
        assert_eq!(
            store.latest_public_wallet()?.map(|w| w.id),
            Some(WalletId("first".to_owned()))
        );
        Ok(())
    }
}
