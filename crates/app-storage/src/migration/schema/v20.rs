//! One irreversible transfer attempt per persisted user turn.
pub(in crate::migration) const MIGRATION_20: &str = "
CREATE TABLE chat_transfer_attempts (
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    -- Message rows are replaced when history is saved; retain the turn tombstone.
    message_id TEXT NOT NULL CHECK (length(message_id) BETWEEN 1 AND 160),
    PRIMARY KEY (conversation_id, message_id)
) STRICT;
INSERT INTO schema_migrations (version, name) VALUES (20, 'chat_transfer_attempts');
";
