//! Durable reservation/dispatch distinction for crash recovery.
pub(in crate::migration) const MIGRATION_21: &str = "
CREATE TABLE evm_transfer_dispatches (
    transaction_hash TEXT PRIMARY KEY NOT NULL REFERENCES evm_transactions(transaction_hash) ON DELETE CASCADE,
    reservation_id TEXT NOT NULL CHECK (length(reservation_id) BETWEEN 1 AND 160),
    phase TEXT NOT NULL CHECK (phase IN ('reserved','dispatched','acknowledged'))
) STRICT;
INSERT INTO schema_migrations (version,name) VALUES (21,'evm_transfer_dispatches');
";
