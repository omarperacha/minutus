pub(in crate::migration) const MIGRATION_10: &str = r"
CREATE TABLE semantic_compaction_checkpoints (
    conversation_id TEXT PRIMARY KEY
        REFERENCES conversations(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    covered_message_count INTEGER NOT NULL CHECK (covered_message_count > 0),
    covered_message_id TEXT NOT NULL CHECK (length(covered_message_id) BETWEEN 1 AND 160),
    history_sha256 TEXT NOT NULL CHECK (length(history_sha256) = 64),
    source_state_sha256 TEXT NOT NULL CHECK (length(source_state_sha256) = 64),
    summary_policy_version INTEGER NOT NULL CHECK (summary_policy_version > 0),
    model_sha256 TEXT NOT NULL CHECK (length(model_sha256) = 64),
    checkpoint_json TEXT NOT NULL CHECK (
        json_valid(checkpoint_json) AND length(checkpoint_json) BETWEEN 2 AND 16384
    ),
    updated_at TEXT NOT NULL CHECK (length(updated_at) BETWEEN 1 AND 64)
) STRICT;

CREATE TRIGGER semantic_compaction_project_scope_insert
BEFORE INSERT ON semantic_compaction_checkpoints
WHEN NOT EXISTS (
    SELECT 1 FROM conversations
    WHERE conversations.id = NEW.conversation_id
      AND conversations.project_id = NEW.project_id
)
BEGIN
    SELECT RAISE(ABORT, 'semantic checkpoint project scope mismatch');
END;

CREATE TRIGGER semantic_compaction_project_scope_update
BEFORE UPDATE OF conversation_id, project_id ON semantic_compaction_checkpoints
WHEN NOT EXISTS (
    SELECT 1 FROM conversations
    WHERE conversations.id = NEW.conversation_id
      AND conversations.project_id = NEW.project_id
)
BEGIN
    SELECT RAISE(ABORT, 'semantic checkpoint project scope mismatch');
END;

CREATE INDEX semantic_compaction_checkpoints_project
ON semantic_compaction_checkpoints(project_id, updated_at);

INSERT INTO schema_migrations (version, name)
VALUES (10, 'semantic_compaction_checkpoints');
";

pub(in crate::migration) const MIGRATION_11: &str = r"
ALTER TABLE audit_events RENAME TO audit_events_legacy;

CREATE TABLE audit_events (
    id TEXT PRIMARY KEY CHECK (length(id) > 0),
    event_kind TEXT NOT NULL CHECK (event_kind IN (
        'application_opened',
        'project_created',
        'workspace_granted',
        'workspace_permission_changed',
        'workspace_revoked',
        'filesystem_tool',
        'model_registered',
        'conversation_deleted'
    )),
    outcome TEXT NOT NULL CHECK (outcome IN ('succeeded', 'denied', 'failed')),
    subject_id TEXT,
    operation_id TEXT CHECK (operation_id IS NULL OR length(operation_id) > 0),
    tool_name TEXT CHECK (tool_name IS NULL OR tool_name IN (
        'filesystem.list',
        'filesystem.find_paths',
        'filesystem.stat',
        'filesystem.read_text',
        'filesystem.inspect_image',
        'filesystem.search',
        'filesystem.scan',
        'filesystem.write_text',
        'filesystem.replace_text',
        'filesystem.undo_text',
        'filesystem.undo_available'
    )),
    workspace_grant_id TEXT CHECK (
        workspace_grant_id IS NULL OR length(workspace_grant_id) > 0
    ),
    relative_path TEXT CHECK (
        relative_path IS NULL OR (
            length(relative_path) BETWEEN 1 AND 1024 AND
            substr(relative_path, 1, 1) NOT IN ('/', char(92)) AND
            instr(relative_path, char(0)) = 0
        )
    ),
    permission_decision TEXT NOT NULL CHECK (
        permission_decision IN ('allowed', 'denied', 'not_applicable')
    ),
    result_count INTEGER CHECK (result_count IS NULL OR result_count >= 0),
    truncated INTEGER CHECK (truncated IS NULL OR truncated IN (0, 1)),
    occurred_at TEXT NOT NULL,
    CHECK (
        event_kind != 'filesystem_tool' OR
        (tool_name IS NOT NULL AND workspace_grant_id IS NOT NULL)
    )
) STRICT;

INSERT INTO audit_events (
    id, event_kind, outcome, subject_id, operation_id, tool_name,
    workspace_grant_id, relative_path, permission_decision,
    result_count, truncated, occurred_at
)
SELECT
    id, event_kind, outcome, subject_id, operation_id, tool_name,
    workspace_grant_id, relative_path, permission_decision,
    result_count, truncated, occurred_at
FROM audit_events_legacy;

DROP TABLE audit_events_legacy;

CREATE INDEX audit_events_occurred_at ON audit_events(occurred_at, id);
CREATE INDEX audit_events_workspace_grant ON audit_events(workspace_grant_id, occurred_at);

INSERT INTO schema_migrations (version, name)
VALUES (11, 'filesystem_path_discovery_audit');
";

pub(in crate::migration) const MIGRATION_12: &str = r"
ALTER TABLE conversations ADD COLUMN model_id TEXT
    CHECK (model_id IS NULL OR length(model_id) BETWEEN 1 AND 256);
ALTER TABLE conversations ADD COLUMN model_version TEXT
    CHECK (model_version IS NULL OR length(model_version) BETWEEN 1 AND 256);

CREATE TRIGGER conversations_model_pair_insert
BEFORE INSERT ON conversations
WHEN (NEW.model_id IS NULL) != (NEW.model_version IS NULL)
BEGIN
    SELECT RAISE(ABORT, 'conversation model identity must be complete');
END;

CREATE TRIGGER conversations_model_pair_update
BEFORE UPDATE OF model_id, model_version ON conversations
WHEN (NEW.model_id IS NULL) != (NEW.model_version IS NULL)
BEGIN
    SELECT RAISE(ABORT, 'conversation model identity must be complete');
END;

CREATE TABLE provider_model_installs (
    model_id TEXT NOT NULL CHECK (length(model_id) BETWEEN 1 AND 256),
    model_version TEXT NOT NULL CHECK (length(model_version) BETWEEN 1 AND 256),
    installed_bytes INTEGER NOT NULL CHECK (installed_bytes > 0),
    installed_at TEXT NOT NULL CHECK (length(installed_at) > 0),
    PRIMARY KEY (model_id, model_version)
) STRICT;

INSERT INTO schema_migrations (version, name)
VALUES (12, 'persist_conversation_model_selection');
";

pub(in crate::migration) const MIGRATION_13: &str = r"
ALTER TABLE projects ADD COLUMN model_id TEXT
    CHECK (model_id IS NULL OR length(model_id) BETWEEN 1 AND 256);
ALTER TABLE projects ADD COLUMN model_version TEXT
    CHECK (model_version IS NULL OR length(model_version) BETWEEN 1 AND 256);

CREATE TRIGGER projects_model_pair_insert
BEFORE INSERT ON projects
WHEN (NEW.model_id IS NULL) != (NEW.model_version IS NULL)
BEGIN
    SELECT RAISE(ABORT, 'project model identity must be complete');
END;

CREATE TRIGGER projects_model_pair_update
BEFORE UPDATE OF model_id, model_version ON projects
WHEN (NEW.model_id IS NULL) != (NEW.model_version IS NULL)
BEGIN
    SELECT RAISE(ABORT, 'project model identity must be complete');
END;

INSERT INTO schema_migrations (version, name)
VALUES (13, 'persist_project_model_selection');
";

pub(in crate::migration) const MIGRATION_14: &str = r"
CREATE TABLE rich_evidence_checkpoints (
    conversation_id TEXT PRIMARY KEY
        REFERENCES conversations(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    covered_message_count INTEGER NOT NULL CHECK (covered_message_count > 0),
    covered_message_id TEXT NOT NULL CHECK (length(covered_message_id) BETWEEN 1 AND 160),
    history_sha256 TEXT NOT NULL CHECK (length(history_sha256) = 64),
    source_state_sha256 TEXT NOT NULL CHECK (length(source_state_sha256) = 64),
    extraction_policy_version INTEGER NOT NULL CHECK (extraction_policy_version > 0),
    binding_sha256 TEXT NOT NULL CHECK (length(binding_sha256) = 64),
    checkpoint_json TEXT NOT NULL CHECK (
        json_valid(checkpoint_json) AND length(checkpoint_json) BETWEEN 2 AND 16384
    ),
    updated_at TEXT NOT NULL CHECK (length(updated_at) BETWEEN 1 AND 64)
) STRICT;

CREATE TRIGGER rich_evidence_project_scope_insert
BEFORE INSERT ON rich_evidence_checkpoints
WHEN NOT EXISTS (
    SELECT 1 FROM conversations
    WHERE conversations.id = NEW.conversation_id
      AND conversations.project_id = NEW.project_id
)
BEGIN
    SELECT RAISE(ABORT, 'rich evidence checkpoint project scope mismatch');
END;

CREATE TRIGGER rich_evidence_project_scope_update
BEFORE UPDATE OF conversation_id, project_id ON rich_evidence_checkpoints
WHEN NOT EXISTS (
    SELECT 1 FROM conversations
    WHERE conversations.id = NEW.conversation_id
      AND conversations.project_id = NEW.project_id
)
BEGIN
    SELECT RAISE(ABORT, 'rich evidence checkpoint project scope mismatch');
END;

CREATE INDEX rich_evidence_checkpoints_project
ON rich_evidence_checkpoints(project_id, updated_at);

INSERT INTO schema_migrations (version, name)
VALUES (14, 'rich_evidence_checkpoints');
";

pub(in crate::migration) const MIGRATION_15: &str = r"
CREATE TABLE evm_transactions (
    transaction_hash TEXT PRIMARY KEY CHECK (
        length(transaction_hash) = 66 AND substr(transaction_hash, 1, 2) = '0x'
    ),
    network TEXT NOT NULL CHECK (network IN ('eip155:8453', 'eip155:84532')),
    sender TEXT NOT NULL CHECK (length(sender) = 42),
    nonce TEXT NOT NULL CHECK (length(nonce) BETWEEN 1 AND 78),
    candidate_digest TEXT NOT NULL CHECK (
        length(candidate_digest) = 66 AND substr(candidate_digest, 1, 2) = '0x'
    ),
    state TEXT NOT NULL CHECK (state IN (
        'pending', 'confirmed', 'failed', 'dropped_or_replaced'
    )),
    block_number TEXT CHECK (block_number IS NULL OR length(block_number) BETWEEN 1 AND 78),
    block_hash TEXT CHECK (
        block_hash IS NULL OR
        (length(block_hash) = 66 AND substr(block_hash, 1, 2) = '0x')
    ),
    submitted_at TEXT NOT NULL CHECK (length(submitted_at) BETWEEN 1 AND 64),
    updated_at TEXT NOT NULL CHECK (length(updated_at) BETWEEN 1 AND 64),
    CHECK ((block_number IS NULL) = (block_hash IS NULL)),
    CHECK (
        (state IN ('confirmed', 'failed') AND block_number IS NOT NULL) OR
        (state IN ('pending', 'dropped_or_replaced') AND block_number IS NULL)
    )
) STRICT;

CREATE INDEX evm_transactions_updated_at
ON evm_transactions(updated_at DESC, transaction_hash);

INSERT INTO schema_migrations (version, name)
VALUES (15, 'persist_evm_transaction_monitoring');
";

pub(in crate::migration) const MIGRATION_16: &str = r"
ALTER TABLE evm_transactions RENAME TO evm_transactions_base_only;

CREATE TABLE evm_transactions (
    transaction_hash TEXT PRIMARY KEY CHECK (
        length(transaction_hash) = 66 AND substr(transaction_hash, 1, 2) = '0x'
    ),
    network TEXT NOT NULL CHECK (network IN (
        'eip155:1', 'eip155:11155111', 'eip155:8453', 'eip155:84532'
    )),
    sender TEXT NOT NULL CHECK (length(sender) = 42),
    nonce TEXT NOT NULL CHECK (length(nonce) BETWEEN 1 AND 78),
    candidate_digest TEXT NOT NULL CHECK (
        length(candidate_digest) = 66 AND substr(candidate_digest, 1, 2) = '0x'
    ),
    state TEXT NOT NULL CHECK (state IN (
        'pending', 'confirmed', 'failed', 'dropped_or_replaced'
    )),
    block_number TEXT CHECK (block_number IS NULL OR length(block_number) BETWEEN 1 AND 78),
    block_hash TEXT CHECK (
        block_hash IS NULL OR
        (length(block_hash) = 66 AND substr(block_hash, 1, 2) = '0x')
    ),
    submitted_at TEXT NOT NULL CHECK (length(submitted_at) BETWEEN 1 AND 64),
    updated_at TEXT NOT NULL CHECK (length(updated_at) BETWEEN 1 AND 64),
    CHECK ((block_number IS NULL) = (block_hash IS NULL)),
    CHECK (
        (state IN ('confirmed', 'failed') AND block_number IS NOT NULL) OR
        (state IN ('pending', 'dropped_or_replaced') AND block_number IS NULL)
    )
) STRICT;

INSERT INTO evm_transactions
SELECT * FROM evm_transactions_base_only;

DROP TABLE evm_transactions_base_only;

CREATE INDEX evm_transactions_updated_at
ON evm_transactions(updated_at DESC, transaction_hash);

INSERT INTO schema_migrations (version, name)
VALUES (16, 'support_ethereum_transaction_monitoring');
";

pub(in crate::migration) const MIGRATION_17: &str = r"
CREATE TABLE transaction_approval_finalizations (
    approval_id TEXT PRIMARY KEY CHECK (
        length(approval_id) = 64 AND
        approval_id NOT GLOB '*[^0-9a-f]*'
    ),
    commitment_digest TEXT NOT NULL CHECK (
        length(commitment_digest) = 64 AND
        commitment_digest NOT GLOB '*[^0-9a-f]*'
    ),
    signing_digest TEXT NOT NULL CHECK (
        length(signing_digest) = 64 AND
        signing_digest NOT GLOB '*[^0-9a-f]*'
    ),
    chain_id TEXT NOT NULL CHECK (
        length(chain_id) BETWEEN 1 AND 20 AND
        chain_id NOT GLOB '*[^0-9]*' AND
        (chain_id = '0' OR substr(chain_id, 1, 1) != '0')
    ),
    account TEXT NOT NULL CHECK (
        length(account) = 42 AND substr(account, 1, 2) = '0x'
    ),
    session_id TEXT NOT NULL CHECK (length(session_id) BETWEEN 1 AND 128),
    expires_at_unix TEXT NOT NULL CHECK (
        length(expires_at_unix) BETWEEN 1 AND 20 AND
        expires_at_unix NOT GLOB '*[^0-9]*'
    ),
    transaction_type TEXT NOT NULL CHECK (transaction_type IN ('eip1559:2', 'eip712')),
    policy_revision TEXT NOT NULL CHECK (
        length(policy_revision) BETWEEN 1 AND 10 AND
        policy_revision NOT GLOB '*[^0-9]*'
    ),
    disposition TEXT NOT NULL CHECK (disposition IN ('consumed', 'invalidated')),
    invalidation_reason TEXT CHECK (
        invalidation_reason IS NULL OR
        invalidation_reason IN ('cancelled', 'expired', 'failed', 'superseded')
    ),
    finalized_at_unix TEXT NOT NULL CHECK (
        length(finalized_at_unix) BETWEEN 1 AND 20 AND
        finalized_at_unix NOT GLOB '*[^0-9]*'
    ),
    CHECK (
        (disposition = 'consumed' AND invalidation_reason IS NULL) OR
        (disposition = 'invalidated' AND invalidation_reason IS NOT NULL)
    )
) STRICT;

CREATE TABLE transaction_approval_replay_events (
    event_id INTEGER PRIMARY KEY,
    approval_id TEXT NOT NULL REFERENCES transaction_approval_finalizations(approval_id),
    attempted_commitment_digest TEXT NOT NULL CHECK (
        length(attempted_commitment_digest) = 64 AND
        attempted_commitment_digest NOT GLOB '*[^0-9a-f]*'
    ),
    outcome TEXT NOT NULL CHECK (outcome IN (
        'already_consumed', 'invalidated', 'identifier_collision'
    )),
    occurred_at_unix TEXT NOT NULL CHECK (
        length(occurred_at_unix) BETWEEN 1 AND 20 AND
        occurred_at_unix NOT GLOB '*[^0-9]*'
    )
) STRICT;

CREATE INDEX transaction_approval_replay_events_approval
ON transaction_approval_replay_events(approval_id, event_id);

INSERT INTO schema_migrations (version, name)
VALUES (17, 'single_use_transaction_approvals');
";
