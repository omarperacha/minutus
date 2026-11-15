//! Dormant sequential contract workflow journal. No signing authority is stored.
pub(in crate::migration) const MIGRATION_22: &str = "
CREATE TABLE contract_workflows (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    message_id TEXT NOT NULL CHECK (length(message_id) BETWEEN 1 AND 160),
    scope_commitment TEXT NOT NULL CHECK (length(scope_commitment) = 64),
    step_commitments_json TEXT NOT NULL,
    UNIQUE (conversation_id, message_id),
    FOREIGN KEY (conversation_id, message_id) REFERENCES chat_transfer_attempts(conversation_id, message_id)
) STRICT;
CREATE TABLE contract_workflow_steps (
    workflow_id INTEGER NOT NULL REFERENCES contract_workflows(id) ON DELETE CASCADE,
    step_index INTEGER NOT NULL CHECK (step_index BETWEEN 0 AND 15),
    commitment TEXT NOT NULL CHECK (length(commitment) = 64),
    state TEXT NOT NULL CHECK (state IN ('planned','reviewing','dispatching','submitted','confirmed_unverified','verified','denied','cancelled','failed','uncertain','interrupted')),
    transaction_hash TEXT CHECK (transaction_hash IS NULL OR length(transaction_hash) = 66),
    evidence_commitment TEXT CHECK (evidence_commitment IS NULL OR length(evidence_commitment) = 64),
    PRIMARY KEY (workflow_id, step_index)
) STRICT;
INSERT INTO schema_migrations (version, name) VALUES (22, 'contract_workflow_journal');
";
